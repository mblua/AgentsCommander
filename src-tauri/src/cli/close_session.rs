use clap::Args;
use std::path::PathBuf;
use uuid::Uuid;

use crate::config::teams;
use crate::phone::types::OutboxMessage;

use super::send::agent_name_from_root;

#[derive(Args)]
#[command(after_help = "\
AUTHORIZATION: Only coordinators of the target agent's team can close sessions. \
The master/root token bypasses this check.\n\n\
BEHAVIOR: By default, graceful shutdown is used — an exit command is injected into \
the agent's PTY (e.g., /exit for Claude Code) and the system waits for clean exit. \
If the agent doesn't exit within --timeout seconds, it falls back to force-kill. \
Use --force to skip graceful shutdown and kill immediately.\n\n\
DISCOVERY: Use `list-peers` to get valid agent names for --target.")]
pub struct CloseSessionArgs {
    /// Session token for authentication (from '# === Session Credentials ===' block)
    #[arg(long)]
    pub token: Option<String>,

    /// Agent root directory (required). Your working directory — used to derive your agent name
    #[arg(long)]
    pub root: Option<String>,

    /// Target agent name to close (e.g., "wg-1-ac-devs/dev-rust"). Use `list-peers` to discover names
    #[arg(long)]
    pub target: String,

    /// Force-kill immediately, skipping graceful shutdown
    #[arg(long)]
    pub force: bool,

    /// Graceful shutdown timeout in seconds per session (default: 30)
    #[arg(long, default_value = "30")]
    pub timeout: u32,
}

pub fn execute(args: CloseSessionArgs) -> i32 {
    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            eprintln!("Error: --root is required. Specify your agent's root directory.");
            return 1;
        }
    };

    // Validate token
    let is_root = match crate::cli::validate_cli_token(&args.token) {
        Ok((_token, root)) => root,
        Err(msg) => {
            eprintln!("{}", msg);
            return 1;
        }
    };

    let sender = agent_name_from_root(&root);

    // Resolve --target against known projects (Decision 2 / §AR2-shared).
    // Belt-and-braces alongside the mailbox-side resolver at handle_close_session
    // entry (§AR2-G1). Fail-fast at the CLI gives users immediate feedback on
    // ambiguous or unknown targets without writing to the outbox.
    let settings = crate::config::settings::load_settings();
    let resolved_target = match crate::config::teams::resolve_agent_target(
        &args.target,
        &settings.project_paths,
    ) {
        Ok(fqn) => fqn,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    // Pre-validate coordinator authorization.
    // Check master token from LocalDir as additional bypass (independent of validate_cli_token).
    let is_master = is_root || {
        if let Some(ref token_str) = args.token {
            crate::config::config_dir()
                .map(|d| d.join("master-token.txt"))
                .and_then(|p| std::fs::read_to_string(&p).ok())
                .map(|m| m.trim() == token_str)
                .unwrap_or(false)
        } else {
            false
        }
    };

    if !is_master {
        let discovered = teams::discover_teams();
        if discovered.is_empty() || !teams::is_coordinator_of(&sender, &resolved_target, &discovered) {
            eprintln!(
                "Error: authorization denied — '{}' is not a coordinator of '{}'. \
                 Only coordinators can close sessions of their team agents.",
                sender, resolved_target
            );
            return 1;
        }
    }

    let msg_id = Uuid::new_v4().to_string();
    let request_id = Uuid::new_v4().to_string();

    let message = OutboxMessage {
        id: msg_id.clone(),
        token: args.token,
        from: sender.clone(),
        to: resolved_target.clone(),
        body: String::new(),
        mode: String::new(),
        get_output: false,
        request_id: Some(request_id.clone()),
        sender_agent: None,
        preferred_agent: String::new(),
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: None,
        action: Some("close-session".to_string()),
        target: Some(resolved_target),
        force: Some(args.force),
        timeout_secs: Some(args.timeout),
    };

    // Write to outbox — use app outbox for root/master token, else agent's outbox
    let ac_dir = PathBuf::from(&root).join(crate::config::agent_local_dir_name());
    let outbox_dir = if is_root {
        let app_outbox = crate::config::config_dir()
            .map(|d| d.join("app-outbox-path.txt"))
            .and_then(|p| std::fs::read_to_string(&p).ok())
            .map(|s| PathBuf::from(s.trim()));
        match app_outbox {
            Some(p) if p.is_dir() => p,
            _ => ac_dir.join("outbox"),
        }
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

    // Poll for delivery confirmation
    let delivered_path = outbox_dir.join("delivered").join(format!("{}.json", msg_id));
    let rejected_reason_path = outbox_dir.join("rejected").join(format!("{}.reason.txt", msg_id));

    let confirm_timeout = std::time::Duration::from_secs(30);
    let confirm_poll = std::time::Duration::from_millis(250);
    let start = std::time::Instant::now();

    loop {
        if delivered_path.exists() {
            break;
        }
        if rejected_reason_path.exists() {
            let reason = std::fs::read_to_string(&rejected_reason_path)
                .unwrap_or_else(|_| "unknown reason".to_string());
            eprintln!("Error: close-session rejected — {}", reason.trim());
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

    // Wait for response with session details.
    // Timeout must exceed graceful shutdown timeout + processing overhead.
    let responses_dir = ac_dir.join("responses");
    let response_path = responses_dir.join(format!("{}.json", request_id));
    let resp_timeout = std::time::Duration::from_secs((args.timeout + 15) as u64);
    let resp_poll = std::time::Duration::from_millis(500);
    let resp_start = std::time::Instant::now();

    loop {
        if response_path.exists() {
            match std::fs::read_to_string(&response_path) {
                Ok(content) => {
                    println!("{}", content);
                    // Parse response: exit 1 if no sessions were actually closed
                    if let Ok(resp) = serde_json::from_str::<serde_json::Value>(&content) {
                        let closed = resp.get("sessions_closed")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        if closed == 0 {
                            return 1;
                        }
                    }
                    return 0;
                }
                Err(e) => {
                    eprintln!("Error: failed to read response: {}", e);
                    return 1;
                }
            }
        }
        if resp_start.elapsed() >= resp_timeout {
            // Delivery succeeded but response timed out — sessions were likely closed
            println!("close-session delivered but response timed out (sessions may have been closed)");
            return 0;
        }
        std::thread::sleep(resp_poll);
    }
}
