use clap::Args;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use crate::config::dark_factory;
use crate::phone::manager::can_communicate;

#[derive(Args)]
pub struct SendArgs {
    /// Session token for authentication
    #[arg(long)]
    pub token: Option<String>,

    /// Destination agent name (e.g., "0_repos/project_x")
    #[arg(long)]
    pub to: String,

    /// Message body
    #[arg(long)]
    pub message: String,

    /// Delivery mode: active-only, wake, wake-and-sleep
    #[arg(long, default_value = "wake")]
    pub mode: String,

    /// Wait for and return the agent's response
    #[arg(long)]
    pub get_output: bool,

    /// Remote command to execute on the agent's PTY (e.g., "clear", "compact")
    #[arg(long)]
    pub command: Option<String>,

    /// Agent CLI to use for wake-and-sleep (default: auto)
    #[arg(long, default_value = "auto")]
    pub agent: String,

    /// Timeout in seconds for --get-output (default: 300)
    #[arg(long, default_value = "300")]
    pub timeout: u64,

    /// Agent root directory (required)
    #[arg(long)]
    pub root: Option<String>,

    /// Write message to a specific outbox directory (e.g., app-outbox path)
    /// instead of <root>/.agentscommander/outbox/
    #[arg(long)]
    pub outbox: Option<String>,
}

/// Outbox message written to .agentscommander/outbox/<uuid>.json
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxMessage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub from: String,
    pub to: String,
    pub body: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub get_output: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_agent: Option<String>,
    #[serde(default)]
    pub preferred_agent: String,
    #[serde(default)]
    pub priority: String,
    pub timestamp: String,
    /// Remote command to execute on agent's PTY (e.g., "clear", "compact")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// Derive agent name from a path: last two components → "parent/folder"
fn agent_name_from_root(root: &str) -> String {
    let normalized = root.replace('\\', "/");
    let components: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if components.len() >= 2 {
        format!(
            "{}/{}",
            components[components.len() - 2],
            components[components.len() - 1]
        )
    } else {
        normalized
    }
}

pub fn execute(args: SendArgs) -> i32 {
    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            eprintln!("Error: --root is required. Specify your agent's root directory.");
            return 1;
        }
    };
    let sender = agent_name_from_root(&root);
    let ac_dir = PathBuf::from(&root).join(".agentscommander");

    // Validate mode — "queue" is no longer supported
    let valid_modes = ["active-only", "wake", "wake-and-sleep"];
    if !valid_modes.contains(&args.mode.as_str()) {
        eprintln!(
            "Error: invalid mode '{}'. Valid: {}",
            args.mode,
            valid_modes.join(", ")
        );
        return 1;
    }

    // ── Pre-validate routing ──────────────────────────────────────────────
    // Load teams config and check if sender can reach destination BEFORE
    // writing to outbox. Fail immediately with a clear error if not.
    let config = dark_factory::load_dark_factory();
    if config.teams.is_empty() || !can_communicate(&sender, &args.to, &config) {
        eprintln!(
            "Error: routing rejected — '{}' cannot reach '{}'. \
             Check team membership and coordinator rules.",
            sender, args.to
        );
        return 1;
    }

    // Validate --command if present
    const ALLOWED_COMMANDS: &[&str] = &["clear", "compact"];
    if let Some(ref cmd) = args.command {
        if !ALLOWED_COMMANDS.contains(&cmd.as_str()) {
            eprintln!(
                "Error: unsupported command '{}'. Allowed: {}",
                cmd,
                ALLOWED_COMMANDS.join(", ")
            );
            return 1;
        }
    }

    let msg_id = Uuid::new_v4().to_string();
    let request_id = if args.get_output {
        Some(Uuid::new_v4().to_string())
    } else {
        None
    };

    let message = OutboxMessage {
        id: msg_id.clone(),
        token: args.token,
        from: sender.clone(),
        to: args.to.clone(),
        body: args.message,
        mode: args.mode,
        get_output: args.get_output,
        request_id: request_id.clone(),
        sender_agent: None,
        preferred_agent: args.agent,
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: args.command,
    };

    // Write to --outbox if specified, otherwise <root>/.agentscommander/outbox/
    let outbox_dir = if let Some(ref outbox_path) = args.outbox {
        PathBuf::from(outbox_path)
    } else {
        ac_dir.join("outbox")
    };
    if let Err(e) = std::fs::create_dir_all(&outbox_dir) {
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

    if let Err(e) = std::fs::write(&outbox_path, json) {
        eprintln!("Error: failed to write outbox file: {}", e);
        return 1;
    }

    // ── Poll for delivery confirmation ────────────────────────────────────
    // The MailboxPoller will pick up the file and move it to delivered/ or
    // rejected/. Wait until we know the outcome.
    let delivered_path = outbox_dir.join("delivered").join(format!("{}.json", msg_id));
    let rejected_reason_path = outbox_dir.join("rejected").join(format!("{}.reason.txt", msg_id));

    let confirm_timeout = std::time::Duration::from_secs(30);
    let confirm_poll = std::time::Duration::from_millis(250);
    let start = std::time::Instant::now();

    loop {
        if delivered_path.exists() {
            println!("Message delivered: {}", msg_id);
            break;
        }
        if rejected_reason_path.exists() {
            let reason = std::fs::read_to_string(&rejected_reason_path)
                .unwrap_or_else(|_| "unknown reason".to_string());
            eprintln!("Error: message rejected — {}", reason.trim());
            return 1;
        }
        if start.elapsed() >= confirm_timeout {
            eprintln!(
                "Error: delivery confirmation timeout after 30s (message {} may still be pending)",
                msg_id
            );
            return 1;
        }
        std::thread::sleep(confirm_poll);
    }

    // ── If --get-output, wait for response after confirmed delivery ───────
    if let Some(rid) = request_id {
        let responses_dir = ac_dir.join("responses");
        let response_path = responses_dir.join(format!("{}.json", rid));
        let timeout = std::time::Duration::from_secs(args.timeout);
        let poll_interval = std::time::Duration::from_secs(2);
        let resp_start = std::time::Instant::now();

        println!("Waiting for response (timeout: {}s)...", args.timeout);

        loop {
            if resp_start.elapsed() >= timeout {
                eprintln!("Error: timeout waiting for response after {}s", args.timeout);
                return 1;
            }

            if response_path.exists() {
                match std::fs::read_to_string(&response_path) {
                    Ok(content) => {
                        println!("{}", content);
                        return 0;
                    }
                    Err(e) => {
                        eprintln!("Error: failed to read response file: {}", e);
                        return 1;
                    }
                }
            }

            std::thread::sleep(poll_interval);
        }
    }

    0
}
