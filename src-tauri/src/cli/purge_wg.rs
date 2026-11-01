use clap::Args;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::filesystem::{directories, documents};
use crate::phone::types::OutboxMessage;

use super::send::agent_name_from_root;

/// Pure: decide CLI exit code from the daemon's response body.
/// Exit codes:
///   0 - purged (or dry_run_ready)
///   3 - gate rejected (busy/restore_in_progress/dry_run_blocked)
///   4 - a destroy failed after the gate passed (partial_failure/failed_root_guard)
///   2 - unparseable JSON, missing/unknown/non-string status (outcome unknown)
///   1 - auth/IO error or rejected (handled by the rejected/ path)
fn interpret_purge_response_exit_code(content: &str) -> i32 {
    let resp: serde_json::Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => return 2,
    };
    let Some(status) = resp.get("status").and_then(|v| v.as_str()) else {
        return 2;
    };
    match status {
        "purged" | "dry_run_ready" => 0,
        "rejected_busy" | "restore_in_progress" | "dry_run_blocked" => 3,
        "partial_failure" | "failed_root_guard" => 4,
        _ => 2,
    }
}

/// Print a human-readable status line after the JSON response.
fn print_status_prose(content: &str) {
    let Ok(resp) = serde_json::from_str::<serde_json::Value>(content) else {
        return;
    };
    let status = resp.get("status").and_then(|v| v.as_str()).unwrap_or("");
    match status {
        "rejected_busy" => {
            crate::cli_println!(
                "Purge rejected: one or more peers are not purgeable. See per-peer table above."
            );
        }
        "dry_run_ready" => {
            crate::cli_println!("Dry-run: gate would PASS. No sessions destroyed.");
        }
        "dry_run_blocked" => {
            crate::cli_println!("Dry-run: gate would REJECT. One or more peers are not purgeable.");
        }
        "restore_in_progress" => {
            crate::cli_println!("Daemon is still restoring sessions; retry in a few seconds.");
        }
        "partial_failure" => {
            crate::cli_println!("Purge partially failed: some sessions could not be destroyed.");
        }
        "failed_root_guard" => {
            let sid = resp
                .get("offending_session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let cwd = resp
                .get("offending_working_directory")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            crate::cli_println!(
                "Root guard tripped: session {} at '{}' appears to be a Root Agent record. \
                 Nothing destroyed. Repair sessions.json and retry.",
                sid,
                cwd
            );
        }
        _ => {}
    }
}

#[derive(Args)]
#[command(after_help = "\
SCOPE: Purges every peer in the CALLER'S OWN room. The caller itself is never \
purged, and neither is the Root Agent. Cross-room purge is not supported.\n\n\
AUTHORIZATION: The caller must be the identity-verified orchestrator of its room. \
The master/root token does NOT bypass this: a root token has no room.\n\n\
BUSY GATE: If ANY in-scope peer has produced printable output within --quiet-period-ms, \
the command purges NOBODY and exits 3.\n\n\
EXIT CODES: 0 purged (or dry-run would pass) | 1 auth/IO error | 2 outcome unknown \
| 3 gate rejected (a peer is busy) | 4 a destroy failed after the gate passed.")]
pub struct PurgeWgArgs {
    #[arg(long)]
    pub token: Option<String>,

    #[arg(long)]
    pub root: Option<String>,

    /// Safety assertion, not a scope selector: fail unless the resolved
    /// room has exactly this name.
    #[arg(long = "room", alias = "wg", value_name = "ROOM")]
    pub wg: Option<String>,

    /// Inject an exit command and wait, instead of killing immediately.
    /// Off by default: OpenCode ignores exit and burns the full timeout.
    /// WARNING: --graceful stalls ALL inter-agent messaging daemon-wide for
    /// the duration of the purge (the message poller is sequential).
    #[arg(long)]
    pub graceful: bool,

    /// Graceful shutdown timeout in seconds per session.
    #[arg(long, default_value = "5")]
    pub timeout: u32,

    /// Evaluate the gate and print the per-peer table. Destroy nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Printable-silence a peer must show to be purgeable.
    /// Clamped daemon-side to a floor of 2500.
    #[arg(long, default_value = "3000")]
    pub quiet_period_ms: u64,
}

pub fn execute(args: PurgeWgArgs) -> i32 {
    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            log::error!("--root is required. Specify your agent's root directory.");
            eprintln!("Error: --root is required. Specify your agent's root directory.");
            return 1;
        }
    };

    if let Err(msg) = crate::cli::validate_cli_token(&args.token) {
        log::error!("{}", msg);
        eprintln!("{}", msg);
        return 1;
    }

    if args.graceful {
        eprintln!(
            "WARNING: --graceful stalls all inter-agent messaging daemon-wide for up to \
             {}s per peer. Consider --force (the default) for faster purges.",
            args.timeout
        );
    }

    let sender = agent_name_from_root(&root);

    let msg_id = Uuid::new_v4().to_string();
    let request_id = Uuid::new_v4().to_string();

    let message = OutboxMessage {
        id: msg_id.clone(),
        token: args.token.clone(),
        from: sender.clone(),
        to: String::new(),
        body: String::new(),
        mode: String::new(),
        get_output: false,
        request_id: Some(request_id.clone()),
        sender_agent: None,
        preferred_agent: String::new(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: None,
        action: Some(crate::phone::mailbox::PURGE_WG_ACTION.to_string()),
        target: args.wg.clone(),
        force: Some(!args.graceful),
        timeout_secs: Some(args.timeout),
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: Some(args.dry_run),
        quiet_period_ms: Some(args.quiet_period_ms),
        pty_input: None,
    };

    let ac_dir = PathBuf::from(&root).join(crate::config::agent_local_dir_name());
    let outbox_dir = ac_dir.join("outbox");

    if let Err(e) = directories::ensure_cli_outbox(&outbox_dir) {
        log::error!("failed to create outbox directory: {}", e);
        eprintln!("Error: failed to create outbox directory: {}", e);
        return 1;
    }

    let outbox_path = outbox_dir.join(format!("{}.json", msg_id));
    let json = match serde_json::to_string_pretty(&message) {
        Ok(j) => j,
        Err(e) => {
            log::error!("failed to serialize message: {}", e);
            eprintln!("Error: failed to serialize message: {}", e);
            return 1;
        }
    };

    if let Err(e) = documents::write_cli_outbox_message(&outbox_path, json.as_bytes()) {
        log::error!("failed to write outbox file: {}", e);
        eprintln!("Error: failed to write outbox file: {}", e);
        return 1;
    }

    wait_for_purge_response(&ac_dir, &outbox_dir, &request_id, &msg_id, &args)
}

fn wait_for_purge_response(
    ac_dir: &Path,
    outbox_dir: &Path,
    request_id: &str,
    msg_id: &str,
    args: &PurgeWgArgs,
) -> i32 {
    let response_path = ac_dir
        .join("responses")
        .join(format!("{}.json", request_id));
    let rejected_reason_path = outbox_dir
        .join("rejected")
        .join(format!("{}.reason.txt", msg_id));

    // (#885 F-6) Single deadline: collapse close-session's two poll loops
    // into one. The 30s hardcoded confirm_timeout fires first on graceful
    // purges. move_to_delivered runs at the END of the handler (step 14),
    // so a separate delivery-confirmation window buys nothing and can only
    // fire spuriously.
    const MAX_EXPECTED_PEERS: u64 = 32;
    let deadline = if args.graceful {
        std::time::Duration::from_secs(args.timeout as u64 * MAX_EXPECTED_PEERS + 30)
    } else {
        std::time::Duration::from_secs(90)
    };
    let poll_interval = std::time::Duration::from_millis(500);
    let start = std::time::Instant::now();

    loop {
        if directories::cli_outbox_artifact_exists(&response_path) {
            match documents::read_cli_outbox_text(&response_path) {
                Ok(content) => {
                    crate::cli_println!("{}", content);
                    print_status_prose(&content);
                    return interpret_purge_response_exit_code(&content);
                }
                Err(e) => {
                    log::error!("failed to read response: {}", e);
                    eprintln!("Error: failed to read response: {}", e);
                    return 1;
                }
            }
        }
        if directories::cli_outbox_artifact_exists(&rejected_reason_path) {
            let reason = documents::read_cli_outbox_text(&rejected_reason_path)
                .unwrap_or_else(|_| "unknown reason".to_string());
            let trimmed = reason.trim();
            log::error!("purge-wg rejected - {}", trimmed);
            eprintln!("Error: purge-room rejected - {}", trimmed);
            return 1;
        }
        if start.elapsed() >= deadline {
            log::error!(
                "purge-wg response timeout after {}s (request {} may still be pending)",
                deadline.as_secs(),
                msg_id
            );
            eprintln!(
                "Error: purge-room response timeout after {}s (request {} may still be pending)",
                deadline.as_secs(),
                msg_id
            );
            return 2;
        }
        std::thread::sleep(poll_interval);
    }
}

#[cfg(test)]
mod tests;
