use clap::Args;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

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

    /// Delivery mode: queue, active-only, wake, wake-and-sleep
    #[arg(long, default_value = "queue")]
    pub mode: String,

    /// Wait for and return the agent's response
    #[arg(long)]
    pub get_output: bool,

    /// Agent CLI to use for wake-and-sleep (default: auto)
    #[arg(long, default_value = "auto")]
    pub agent: String,

    /// Timeout in seconds for --get-output (default: 300)
    #[arg(long, default_value = "300")]
    pub timeout: u64,
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
}

/// Resolve the current repo identity from cwd.
/// Uses the last two path components (parent/repo) as the agent name.
fn resolve_sender_name() -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let components: Vec<&str> = cwd
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    if components.len() >= 2 {
        format!(
            "{}/{}",
            components[components.len() - 2],
            components[components.len() - 1]
        )
    } else {
        cwd.to_string_lossy().to_string()
    }
}

/// Find the .agentscommander directory for the current repo (walks up from cwd).
fn find_ac_dir() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let ac = dir.join(".agentscommander");
        if ac.is_dir() {
            return Some(ac);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

pub fn execute(args: SendArgs) -> i32 {
    let sender = resolve_sender_name();

    // Validate mode
    let valid_modes = ["queue", "active-only", "wake", "wake-and-sleep"];
    if !valid_modes.contains(&args.mode.as_str()) {
        eprintln!(
            "Error: invalid mode '{}'. Valid: {}",
            args.mode,
            valid_modes.join(", ")
        );
        return 1;
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
        sender_agent: None, // Will be populated in Step 7
        preferred_agent: args.agent,
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    // Write to .agentscommander/outbox/
    let ac_dir = match find_ac_dir() {
        Some(d) => d,
        None => {
            eprintln!("Error: no .agentscommander directory found in current path hierarchy");
            return 1;
        }
    };

    let outbox_dir = ac_dir.join("outbox");
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

    println!("Message queued: {}", msg_id);

    // If --get-output, enter polling loop for response
    if let Some(rid) = request_id {
        let responses_dir = ac_dir.join("responses");
        let response_path = responses_dir.join(format!("{}.json", rid));
        let timeout = std::time::Duration::from_secs(args.timeout);
        let poll_interval = std::time::Duration::from_secs(2);
        let start = std::time::Instant::now();

        println!("Waiting for response (timeout: {}s)...", args.timeout);

        loop {
            if start.elapsed() >= timeout {
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
