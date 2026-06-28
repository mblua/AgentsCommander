use clap::Args;
use std::path::PathBuf;
use uuid::Uuid;

use crate::phone::types::OutboxMessage;

use super::self_clear::resolve_self_clear_sender;

fn interpret_self_switch_response_exit_code(content: &str) -> i32 {
    let resp: serde_json::Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => return 2,
    };
    match resp.get("status").and_then(|v| v.as_str()) {
        Some("queued") | Some("already_queued") => 0,
        _ => 2,
    }
}

fn normalize_profile_arg(profile: Option<String>) -> Result<Option<String>, String> {
    match profile {
        Some(value) => crate::config::settings::normalize_profile_letter(&value)
            .map(Some)
            .ok_or_else(|| "Error: --profile must be a single letter A through Z.".to_string()),
        None => Ok(None),
    }
}

#[derive(Args)]
#[command(after_help = "\
Hands off, switches the CALLER'S OWN session coding agent and/or profile, respawns it fresh, \
then resumes from SELF-HANDOFF.md.\n\n\
BEFORE invoking, write SELF-HANDOFF.md in your own root with the notes you need to resume. \
If SELF-HANDOFF.md is missing, the command refuses.\n\n\
--coding-agent takes the configured coding-agent entry id from settings, not the backend kind \
or AC peer name. If the id is unknown, the daemon rejection lists configured ids.\n\n\
Omitting both --coding-agent and --profile is allowed. It hard-resets the live running recipe \
by respawning the same coding agent/profile fresh and then injecting the handoff prompt.\n\n\
SCOPE: WG replicas only (__agent_* under a wg-* workgroup). Root Agent and origin matrix agents \
are rejected in v1.")]
pub struct SelfSwitchArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN. Shape-validated in the CLI;
    /// per-session authorization happens at the daemon mailbox.
    #[arg(long)]
    pub token: Option<String>,

    /// Agent root directory (required). Your working directory.
    #[arg(long)]
    pub root: Option<String>,

    /// Target configured coding-agent entry id. Omit to keep the live session's agent.
    #[arg(long = "coding-agent")]
    pub coding_agent: Option<String>,

    /// Target profile slot letter A-Z. Omit to keep the live session's effective profile.
    #[arg(long)]
    pub profile: Option<String>,

    /// Seconds to wait for the daemon's queue acknowledgement (default 15).
    #[arg(long, default_value = "15")]
    pub timeout: u64,
}

pub fn execute(args: SelfSwitchArgs) -> i32 {
    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            eprintln!("Error: --root is required.");
            return 1;
        }
    };
    let switch_profile = match normalize_profile_arg(args.profile) {
        Ok(profile) => profile,
        Err(msg) => {
            eprintln!("{}", msg);
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
        from: sender.clone(),
        to: String::new(),
        body: String::new(),
        mode: String::new(),
        get_output: false,
        request_id: Some(request_id.clone()),
        sender_agent: None,
        preferred_agent: String::new(),
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: None,
        action: Some(crate::phone::mailbox::SELF_SWITCH_ACTION.to_string()),
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: args.coding_agent,
        switch_profile,
    };

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
        if delivered_path.exists() {
            break;
        }
        if rejected_reason_path.exists() {
            let reason = std::fs::read_to_string(&rejected_reason_path)
                .unwrap_or_else(|_| "unknown reason".to_string());
            eprintln!(
                "Error: self-handoff-and-switch rejected - {}",
                reason.trim()
            );
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
        if response_path.exists() {
            match std::fs::read_to_string(&response_path) {
                Ok(content) => {
                    crate::cli_println!("{}", content);
                    match serde_json::from_str::<serde_json::Value>(&content)
                        .ok()
                        .and_then(|v| {
                            v.get("status")
                                .and_then(|s| s.as_str())
                                .map(String::from)
                        })
                        .as_deref()
                    {
                        Some("queued") => crate::cli_println!(
                            "self-handoff-and-switch requested. Phase 1 respawns only after this session is \
                             continuously idle for {0}s; Phase 2 then waits a fresh {0}s of idle in the new \
                             session and injects a prompt to read SELF-HANDOFF.md and resume. Best-effort and \
                             not guaranteed.",
                            crate::phone::mailbox::SELF_CLEAR_SETTLE_SECS
                        ),
                        Some("already_queued") => crate::cli_println!(
                            "A self context operation is already pending for this session. Best-effort; re-issue later if needed."
                        ),
                        _ => {}
                    }
                    return interpret_self_switch_response_exit_code(&content);
                }
                Err(e) => {
                    eprintln!("Error: failed to read response: {}", e);
                    return 1;
                }
            }
        }
        if resp_start.elapsed() >= resp_timeout {
            eprintln!(
                "Error: self-handoff-and-switch delivered but no queue ack within {}s (request {})",
                args.timeout, request_id
            );
            return 2;
        }
        std::thread::sleep(resp_poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_status_returns_zero() {
        let resp = r#"{"action":"self-handoff-and-switch","status":"queued","session_id":"s"}"#;
        assert_eq!(interpret_self_switch_response_exit_code(resp), 0);
    }

    #[test]
    fn already_queued_status_returns_zero() {
        let resp =
            r#"{"action":"self-handoff-and-switch","status":"already_queued","session_id":"s"}"#;
        assert_eq!(interpret_self_switch_response_exit_code(resp), 0);
    }

    #[test]
    fn unknown_status_returns_two() {
        let resp = r#"{"status":"weird","action":"self-handoff-and-switch"}"#;
        assert_eq!(interpret_self_switch_response_exit_code(resp), 2);
    }

    #[test]
    fn normalize_profile_arg_uppercases_valid_letter() {
        assert_eq!(
            normalize_profile_arg(Some("b".into())).unwrap().as_deref(),
            Some("B")
        );
    }

    #[test]
    fn normalize_profile_arg_rejects_non_letter() {
        assert!(normalize_profile_arg(Some("aa".into())).is_err());
        assert!(normalize_profile_arg(Some("1".into())).is_err());
    }

    #[test]
    fn self_switch_args_parse_with_default_timeout() {
        use clap::Parser;
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "self-handoff-and-switch",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--root",
            "anything",
        ])
        .expect("clap should accept self-handoff-and-switch with token + root");
        let cmd = parsed.command.expect("subcommand present");
        match cmd {
            crate::cli::Commands::SelfSwitch(args) => {
                assert_eq!(args.timeout, 15);
                assert_eq!(
                    args.token.as_deref(),
                    Some("11111111-1111-1111-1111-111111111111")
                );
                assert_eq!(args.root.as_deref(), Some("anything"));
                assert_eq!(args.coding_agent, None);
                assert_eq!(args.profile, None);
            }
            _ => panic!("expected SelfSwitch subcommand"),
        }
    }

    #[test]
    fn self_switch_args_parse_coding_agent_and_profile() {
        use clap::Parser;
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "self-handoff-and-switch",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--root",
            "anything",
            "--coding-agent",
            "agent_1719526800000_3",
            "--profile",
            "c",
            "--timeout",
            "42",
        ])
        .expect("clap should accept switch args");
        let cmd = parsed.command.expect("subcommand present");
        match cmd {
            crate::cli::Commands::SelfSwitch(args) => {
                assert_eq!(args.timeout, 42);
                assert_eq!(args.coding_agent.as_deref(), Some("agent_1719526800000_3"));
                assert_eq!(
                    normalize_profile_arg(args.profile).unwrap().as_deref(),
                    Some("C")
                );
            }
            _ => panic!("expected SelfSwitch subcommand"),
        }
    }
}
