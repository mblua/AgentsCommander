use clap::Args;
use std::path::PathBuf;
use uuid::Uuid;

use crate::config::teams;
use crate::phone::types::OutboxMessage;

#[derive(Args)]
#[command(after_help = "\
DELIVERY MODES:\n  \
  wake            Inject into PTY if the destination agent is idle (waiting for input). Reject otherwise.\n  \
  wake-and-sleep  Spawn a temporary session for the destination agent, inject the message, destroy when done.\n\n\
ROUTING: Before delivery, the CLI validates that the sender can reach the destination based on team \
membership and coordinator rules (teams.json). If routing fails, the CLI exits immediately with code 1.\n\n\
DISCOVERY: Use `list-peers` to get valid agent names for --to. The \"name\" field in the JSON output \
is the value to use.\n\n\
FILE-BASED MESSAGING: --send <filename> delivers the file at <workgroup-root>/messaging/<filename> \
to the recipient. The PTY only carries a short notification pointing to the absolute path; the \
recipient reads the file via filesystem, bypassing PTY truncation. Sender MUST write the file before \
invoking this command. Filename must exist in the messaging directory and match the canonical shape: \
YYYYMMDD-HHMMSS-<wgN>-<from>-to-<wgN>-<to>-<slug>[.N].md.")]
pub struct SendArgs {
    /// Session token for authentication (from '# === Session Credentials ===' block)
    #[arg(long)]
    pub token: Option<String>,

    /// Destination agent name (e.g., "repos/my-project"). Use `list-peers` to discover valid names
    #[arg(long)]
    pub to: String,

    /// Filename (not path) of a message file that already exists in
    /// <workgroup-root>/messaging/. Sender writes the file BEFORE calling send.
    /// Cannot be combined with --command.
    #[arg(long, conflicts_with = "command")]
    pub send: Option<String>,

    /// Delivery mode (see DELIVERY MODES below)
    #[arg(long, default_value = "wake")]
    pub mode: String,

    /// Wait for and return the agent's response (blocks until reply or --timeout)
    #[arg(long)]
    pub get_output: bool,

    /// Remote command to execute on the agent's PTY [possible values: clear, compact].
    /// The agent must be idle. Cannot be combined with --send
    #[arg(long)]
    pub command: Option<String>,

    /// Agent CLI to use for wake-and-sleep mode
    #[arg(long, default_value = "auto")]
    pub agent: String,

    /// Timeout in seconds for --get-output
    #[arg(long, default_value = "300")]
    pub timeout: u64,

    /// Agent root directory (required). Your working directory — used to derive your agent name
    #[arg(long)]
    pub root: Option<String>,

    /// Write message to a specific outbox directory instead of <root>/<local-dir>/outbox/
    #[arg(long)]
    pub outbox: Option<String>,
}

/// Strip `__agent_` and `_agent_` prefixes from directory names.
pub(crate) fn strip_agent_prefix(name: &str) -> &str {
    name.strip_prefix("__agent_")
        .or_else(|| name.strip_prefix("_agent_"))
        .unwrap_or(name)
}

/// Derive agent name from a path: last two components -> "parent/folder",
/// stripping `__agent_`/`_agent_` prefixes for consistent WG replica naming.
pub(crate) fn agent_name_from_root(root: &str) -> String {
    let normalized = root.replace('\\', "/");
    let components: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if components.len() >= 2 {
        let parent = components[components.len() - 2];
        let last = strip_agent_prefix(components[components.len() - 1]);
        format!("{}/{}", parent, last)
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
    // Validate token before proceeding
    let is_root = match crate::cli::validate_cli_token(&args.token) {
        Ok((_token, root)) => root,
        Err(msg) => {
            eprintln!("{}", msg);
            return 1;
        }
    };

    let sender = agent_name_from_root(&root);
    let ac_dir = PathBuf::from(&root).join(crate::config::agent_local_dir_name());

    // Validate mode — "queue" is no longer supported
    let valid_modes = ["wake", "wake-and-sleep"];
    if !valid_modes.contains(&args.mode.as_str()) {
        eprintln!(
            "Error: invalid mode '{}'. Valid: {}",
            args.mode,
            valid_modes.join(", ")
        );
        return 1;
    }

    // ── Pre-validate routing ──────────────────────────────────────────────

    if !is_root {
        // Load discovered teams and check if sender can reach destination BEFORE
        // writing to outbox. Fail immediately with a clear error if not.
        let discovered = teams::discover_teams();
        if !teams::can_communicate(&sender, &args.to, &discovered) {
            eprintln!(
                "Error: routing rejected — '{}' cannot reach '{}'. \
                 Check team membership and coordinator rules.",
                sender, args.to
            );
            return 1;
        }
    }

    // --send + --command mutually exclusive (P0-3)
    if args.send.is_some() && args.command.is_some() {
        eprintln!("Error: --send and --command are mutually exclusive");
        return 1;
    }

    // Resolve message body from --send (file-based messaging per plan §4.1 [r2])
    let message_body = if let Some(ref filename) = args.send {
        let agent_root_path = std::path::Path::new(&root);
        let wg_root = match crate::phone::messaging::workgroup_root(agent_root_path) {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "Error: --send requires --root under a wg-<N>-* ancestor; {}",
                    e
                );
                return 1;
            }
        };
        let msg_dir = match crate::phone::messaging::messaging_dir(&wg_root) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Error: failed to resolve messaging dir: {}", e);
                return 1;
            }
        };
        let abs = match crate::phone::messaging::resolve_existing_message(&msg_dir, filename) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Error: {}", e);
                return 1;
            }
        };

        // UNC-strip only at the single emission site (plan §13.4).
        let abs_str = abs.to_string_lossy();
        let abs_display = abs_str.trim_start_matches(r"\\?\");
        let body = format!("Nuevo mensaje: {}. Lee este archivo.", abs_display);

        // Pre-wrap long-body warn (plan §13.4 §7.9).
        if body.len() > 200 {
            log::warn!(
                "[send] notification body length {} is unusually long",
                body.len()
            );
        }

        // PTY_SAFE_MAX clamp (trimmed overhead: the wrap no longer embeds
        // wg_root or bin_path — only `from` and the fixed framing remain).
        let overhead = crate::phone::messaging::PTY_WRAP_FIXED + sender.len();
        if body.len() + overhead > crate::phone::messaging::PTY_SAFE_MAX {
            eprintln!(
                "Error: notification exceeds PTY-safe length (body {} + overhead {} > {}). \
                 Shorten slug or move workgroup to a shallower path.",
                body.len(),
                overhead,
                crate::phone::messaging::PTY_SAFE_MAX
            );
            return 1;
        }

        body
    } else {
        String::new()
    };

    // Require at least --send or --command
    if message_body.is_empty() && args.command.is_none() {
        eprintln!("Error: --send or --command is required");
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
        body: message_body,
        mode: args.mode,
        get_output: args.get_output,
        request_id: request_id.clone(),
        sender_agent: None,
        preferred_agent: args.agent,
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: args.command,
        action: None,
        target: None,
        force: None,
        timeout_secs: None,
    };

    // Write to --outbox if specified, app outbox if root/master token, otherwise <root>/<local_dir>/outbox/
    let outbox_dir = if let Some(ref outbox_path) = args.outbox {
        PathBuf::from(outbox_path)
    } else if is_root {
        // Root/master token: use the app outbox so the MailboxPoller always finds it
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
