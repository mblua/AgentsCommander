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

    let outbox_dir = crate::filesystem::app_outbox_pointer::directory(
        is_root.then(crate::config::config_dir).flatten().as_deref(),
        &ac_dir,
    );
    if let Err(error) = super::outbox_delivery::deliver(&outbox_dir, &message, "raise-hand") {
        eprintln!("Error: {}", error);
        return 1;
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
