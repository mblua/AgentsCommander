use clap::Args;
use std::path::PathBuf;
use uuid::Uuid;

use crate::filesystem::{directories, documents};
use crate::phone::types::OutboxMessage;

use super::self_clear::resolve_self_clear_sender;

#[derive(Args)]
#[command(after_help = "\
Raises this session's Sidebar communication indicator when the caller token belongs to a \
live orchestrator session with a visible TASK.md title slot. The indicator persists across \
app restarts until cleared by real user input to the session.\n\n\
OUTPUT: prints exactly true or false on stdout when the daemon processes the request. \
Infrastructure failures, stale tokens, daemon rejection, delivery timeout, and malformed \
responses exit non-zero and write errors to stderr.")]
pub struct RaiseHandArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN. Shape-validated in the CLI;
    /// per-session authorization happens at the daemon mailbox.
    #[arg(long)]
    pub token: Option<String>,

    /// Agent root directory (required). Your working directory.
    #[arg(long)]
    pub root: Option<String>,

    /// Seconds to wait for the daemon's response (default 15).
    #[arg(long, default_value = "15")]
    pub timeout: u64,
}

fn interpret_raise_hand_response(content: &str) -> Result<bool, i32> {
    let resp: serde_json::Value = serde_json::from_str(content).map_err(|_| 2)?;
    resp.get("raised").and_then(|v| v.as_bool()).ok_or(2)
}

pub fn execute(args: RaiseHandArgs) -> i32 {
    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            eprintln!("Error: --root is required.");
            return 1;
        }
    };

    let is_root = match crate::cli::validate_cli_token(&args.token) {
        Ok((_t, r)) => r,
        Err(msg) => {
            eprintln!("{}", msg);
            return 1;
        }
    };

    let sender = resolve_self_clear_sender(&root);
    let ac_dir = PathBuf::from(&root).join(crate::config::agent_local_dir_name());

    let msg_id = Uuid::new_v4().to_string();
    let request_id = Uuid::new_v4().to_string();

    let message = OutboxMessage {
        id: msg_id.clone(),
        token: args.token,
        from: sender,
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
        action: Some(crate::phone::mailbox::RAISE_HAND_ACTION.to_string()),
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    };

    let outbox_dir = if is_root {
        crate::filesystem::app_outbox_pointer::get(crate::config::config_dir().as_deref())
            .unwrap_or_else(|| ac_dir.join("outbox"))
    } else {
        ac_dir.join("outbox")
    };
    if let Err(e) = directories::ensure_cli_outbox(&outbox_dir) {
        eprintln!("Error: failed to create outbox directory: {}", e);
        return 1;
    }

    let outbox_path = outbox_dir.join(format!("{}.json", msg_id));
    let json = match serde_json::to_string_pretty(&message) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Error: failed to serialize message: {}", e);
            return 1;
        }
    };
    if let Err(e) = documents::write_cli_outbox_message(&outbox_path, json.as_bytes()) {
        eprintln!("Error: failed to write outbox file: {}", e);
        return 1;
    }

    let delivered_path = outbox_dir
        .join("delivered")
        .join(format!("{}.json", msg_id));
    let rejected_reason_path = outbox_dir
        .join("rejected")
        .join(format!("{}.reason.txt", msg_id));
    let confirm_timeout = std::time::Duration::from_secs(30);
    let confirm_poll = std::time::Duration::from_millis(250);
    let start = std::time::Instant::now();
    loop {
        if directories::cli_outbox_artifact_exists(&delivered_path) {
            break;
        }
        if directories::cli_outbox_artifact_exists(&rejected_reason_path) {
            let reason = documents::read_cli_outbox_text(&rejected_reason_path)
                .unwrap_or_else(|_| "unknown reason".to_string());
            eprintln!("Error: raise-hand rejected - {}", reason.trim());
            return 1;
        }
        if start.elapsed() >= confirm_timeout {
            eprintln!(
                "Error: delivery confirmation timeout after 30s (request {} may still be pending)",
                msg_id
            );
            return 1;
        }
        std::thread::sleep(confirm_poll);
    }

    let responses_dir = ac_dir.join("responses");
    let response_path = responses_dir.join(format!("{}.json", request_id));
    let resp_timeout = std::time::Duration::from_secs(args.timeout);
    let resp_poll = std::time::Duration::from_millis(250);
    let resp_start = std::time::Instant::now();
    loop {
        if directories::cli_outbox_artifact_exists(&response_path) {
            match documents::read_cli_outbox_text(&response_path) {
                Ok(content) => match interpret_raise_hand_response(&content) {
                    Ok(raised) => {
                        crate::cli_println!("{}", raised);
                        return 0;
                    }
                    Err(code) => {
                        eprintln!("Error: raise-hand response was malformed.");
                        return code;
                    }
                },
                Err(e) => {
                    eprintln!("Error: failed to read response: {}", e);
                    return 1;
                }
            }
        }
        if resp_start.elapsed() >= resp_timeout {
            eprintln!(
                "Error: raise-hand delivered but no response within {}s (request {})",
                args.timeout, request_id
            );
            return 2;
        }
        std::thread::sleep(resp_poll);
    }
}

#[cfg(test)]
mod tests;
