use clap::Args;
use std::path::PathBuf;
use uuid::Uuid;

use crate::phone::types::OutboxMessage;

use super::send::agent_name_from_root;

/// Pure: map the daemon's self-clear response body to a CLI exit code.
/// 0 = queued | already_queued. 2 = unparseable / missing / unknown status
/// (daemon spoke incoherently). Mirrors close_session::interpret_close_response_exit_code.
fn interpret_self_clear_response_exit_code(content: &str) -> i32 {
    let resp: serde_json::Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => return 2,
    };
    match resp.get("status").and_then(|v| v.as_str()) {
        Some("queued") | Some("already_queued") => 0,
        _ => 2,
    }
}

/// Pure: self-clear is not available for the Root Agent (user-launched, never
/// auto-managed; the user clears it from the UI). Returns the error message if blocked.
fn self_clear_blocked_for_root(is_root_agent: bool) -> Option<&'static str> {
    if is_root_agent {
        Some("self-clear is not available for the Root Agent")
    } else {
        None
    }
}

#[derive(Args)]
#[command(after_help = "\
Requests a /clear of the CALLER'S OWN agent context. The clear is DEFERRED: it executes only \
after the session has been continuously idle for 30 seconds. If the session goes busy again \
during the window, the 30s timer restarts. Returns as soon as the request is queued; it does \
NOT block until the clear runs.\n\n\
IDENTITY: the session to clear is resolved from --token (find_by_token). You can only clear the \
session that owns the token you present; there is no way to clear another agent.\n\n\
BEST-EFFORT: the deferred clear is NOT guaranteed. A perpetually busy/chatty session that never \
reaches 30s sustained idle, or a daemon restart before the window completes, drops the request \
(a greppable warn line is logged on abandon). Re-issue self-clear if your context is still \
present later.\n\n\
SCOPE: only the `clear` command, and only coding-agent CLIs (Claude / Codex / Gemini).")]
pub struct SelfClearArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN. Shape-validated in the CLI;
    /// per-session authorization happens at the daemon mailbox.
    #[arg(long)]
    pub token: Option<String>,

    /// Agent root directory (required). Your working directory.
    #[arg(long)]
    pub root: Option<String>,

    /// Seconds to wait for the daemon's queue acknowledgement (default 15).
    #[arg(long, default_value = "15")]
    pub timeout: u64,
}

pub fn execute(args: SelfClearArgs) -> i32 {
    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            eprintln!("Error: --root is required.");
            return 1;
        }
    };

    // Root Agent is excluded at the CLI for fast feedback (the daemon re-checks
    // authoritatively in handle_self_clear; the CLI guard alone is bypassable).
    if let Some(reason) =
        self_clear_blocked_for_root(crate::config::root_agent::is_root_agent_path(&root))
    {
        eprintln!("Error: {}", reason);
        return 1;
    }

    let is_root = match crate::cli::validate_cli_token(&args.token) {
        Ok((_t, r)) => r,
        Err(msg) => {
            eprintln!("{}", msg);
            return 1;
        }
    };

    let sender = agent_name_from_root(&root);
    let ac_dir = PathBuf::from(&root).join(crate::config::agent_local_dir_name());

    let msg_id = Uuid::new_v4().to_string();
    let request_id = Uuid::new_v4().to_string();

    let message = OutboxMessage {
        id: msg_id.clone(),
        token: args.token,
        from: sender.clone(),
        to: String::new(), // MED-1: empty skips the `if !msg.to.is_empty()` resolution
        // guard in process_message; self-clear resolves by token and never reads `to`.
        body: String::new(),
        mode: String::new(),
        get_output: false,
        request_id: Some(request_id.clone()),
        sender_agent: None,
        preferred_agent: String::new(),
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: None,
        action: Some("self-clear".to_string()),
        target: None,
        force: None,
        timeout_secs: None,
    };

    // Outbox selection mirrors close_session::execute (is_root -> app outbox else agent outbox).
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

    // Poll delivered/ | rejected/ (queue confirmation, 30s) - same loop as close_session::execute.
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
            eprintln!("Error: self-clear rejected - {}", reason.trim());
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

    // Poll response (queue ack). The daemon writes it immediately after queuing,
    // NOT after the clear runs - so this is fast.
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
                    // MED-3: honest, conditional wording - the deferred clear is best-effort,
                    // NOT a near-certainty. The "30" is single-sourced from the daemon const.
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
                            "self-clear requested. It runs ONLY after this session is continuously idle for {}s; \
                             it is best-effort and NOT guaranteed (a busy/chatty session or a daemon restart drops it). \
                             If your context is still present later, re-issue self-clear.",
                            crate::phone::mailbox::SELF_CLEAR_SETTLE_SECS
                        ),
                        Some("already_queued") => crate::cli_println!(
                            "self-clear already pending for this session (or a clear is in flight); \
                             it runs after {}s sustained idle. Best-effort; re-issue later if needed.",
                            crate::phone::mailbox::SELF_CLEAR_SETTLE_SECS
                        ),
                        _ => {}
                    }
                    return interpret_self_clear_response_exit_code(&content);
                }
                Err(e) => {
                    eprintln!("Error: failed to read response: {}", e);
                    return 1;
                }
            }
        }
        if resp_start.elapsed() >= resp_timeout {
            eprintln!(
                "Error: self-clear delivered but no queue ack within {}s (request {})",
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

    // ── interpret_self_clear_response_exit_code (mirror close-session's table) ──

    #[test]
    fn queued_status_returns_zero() {
        let resp = r#"{"action":"self-clear","status":"queued","session_id":"s","settle_secs":30}"#;
        assert_eq!(interpret_self_clear_response_exit_code(resp), 0);
    }

    #[test]
    fn already_queued_status_returns_zero() {
        let resp =
            r#"{"action":"self-clear","status":"already_queued","session_id":"s","settle_secs":30}"#;
        assert_eq!(interpret_self_clear_response_exit_code(resp), 0);
    }

    #[test]
    fn unknown_status_returns_two() {
        let resp = r#"{"status":"weird_new_state","action":"self-clear"}"#;
        assert_eq!(interpret_self_clear_response_exit_code(resp), 2);
    }

    #[test]
    fn unparseable_json_returns_two() {
        assert_eq!(interpret_self_clear_response_exit_code("not json"), 2);
        assert_eq!(interpret_self_clear_response_exit_code(""), 2);
        assert_eq!(interpret_self_clear_response_exit_code("{partial"), 2);
    }

    #[test]
    fn missing_status_field_returns_two() {
        let resp = r#"{"action":"self-clear","session_id":"s"}"#;
        assert_eq!(interpret_self_clear_response_exit_code(resp), 2);
    }

    #[test]
    fn non_string_status_returns_two() {
        let resp = r#"{"status":42,"action":"self-clear"}"#;
        assert_eq!(interpret_self_clear_response_exit_code(resp), 2);
    }

    /// Valid JSON that is not an object (array, scalar, null) still fails the
    /// `.get("status")` lookup and must return exit 2.
    #[test]
    fn non_object_json_returns_two() {
        assert_eq!(interpret_self_clear_response_exit_code("null"), 2);
        assert_eq!(interpret_self_clear_response_exit_code("[1,2,3]"), 2);
        assert_eq!(interpret_self_clear_response_exit_code("\"queued\""), 2);
        assert_eq!(interpret_self_clear_response_exit_code("42"), 2);
        assert_eq!(interpret_self_clear_response_exit_code("true"), 2);
    }

    // ── self_clear_blocked_for_root ──

    #[test]
    fn blocked_for_root_true_returns_message() {
        assert!(self_clear_blocked_for_root(true).is_some());
    }

    #[test]
    fn blocked_for_root_false_returns_none() {
        assert!(self_clear_blocked_for_root(false).is_none());
    }

    // ── clap parse smoke ──

    #[test]
    fn self_clear_args_parse_with_default_timeout() {
        use clap::Parser;
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "self-clear",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--root",
            "anything",
        ])
        .expect("clap should accept self-clear with token + root");
        let cmd = parsed.command.expect("subcommand present");
        match cmd {
            crate::cli::Commands::SelfClear(args) => {
                assert_eq!(args.timeout, 15, "--timeout must default to 15");
                assert_eq!(
                    args.token.as_deref(),
                    Some("11111111-1111-1111-1111-111111111111")
                );
                assert_eq!(args.root.as_deref(), Some("anything"));
            }
            _ => panic!("expected SelfClear subcommand"),
        }
    }

    #[test]
    fn self_clear_args_parse_explicit_timeout() {
        use clap::Parser;
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "self-clear",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--root",
            "anything",
            "--timeout",
            "42",
        ])
        .expect("clap should accept explicit --timeout");
        let cmd = parsed.command.expect("subcommand present");
        match cmd {
            crate::cli::Commands::SelfClear(args) => assert_eq!(args.timeout, 42),
            _ => panic!("expected SelfClear subcommand"),
        }
    }
}
