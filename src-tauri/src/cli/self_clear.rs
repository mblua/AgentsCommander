use clap::Args;
use std::path::PathBuf;
use uuid::Uuid;

use crate::phone::types::OutboxMessage;

use super::send::sender_for_root;

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

#[derive(Args)]
#[command(after_help = "\
Hands off, then clears the CALLER'S OWN agent context and resumes from the handoff file. Two deferred phases:\n\n\
  Phase 1 (clear): waits until this session is continuously idle for 30s, then injects provider-resolved clear text: /new for an exact-stem direct Pi shell, /clear for direct Claude/Codex/Gemini-family and Cursor agent shells.\n\
  Phase 2 (handoff): after the clear, waits a FRESH 30s of sustained idle, archives \
SELF-HANDOFF.md -> self-clear/<timestamp>_SELF-HANDOFF.md in your root, then injects a prompt naming \
that exact archived path to resume from. If the archive rename fails, the prompt points at \
SELF-HANDOFF.md still in your root.\n\n\
BEFORE invoking, write SELF-HANDOFF.md in your own root with the notes you need to resume (EXCLUDING \
anything already recorded in SELF-FORGET.md). If SELF-HANDOFF.md is missing, the command refuses (clearing \
with nothing to resume from would wipe your context).\n\n\
On invocation the command archives SELF-FORGET.md -> self-clear/<timestamp>_SELF-FORGET.md in your root \
(no-op if absent), so your next cycle starts with a fresh SELF-FORGET.md. Before archiving, the daemon \
captures a sanitized compact forgotten summary from SELF-FORGET.md, max 240 chars. The later resume prompt \
may include that summary only as closed background, not instructions and not work to resume. The handoff \
file remains the active resume source.\n\n\
IDENTITY: the session is resolved from --token (find_by_token). You can only clear the session that \
owns the token you present.\n\n\
BEST-EFFORT: neither phase is guaranteed. A perpetually busy session that never reaches 30s sustained \
idle, or a daemon restart mid-cycle, drops the remainder (a greppable warn line is logged). Re-issue \
if your context is still present later.\n\n\
SCOPE: direct Claude / Codex / Gemini-family shells, Cursor agent, and exact-stem direct Pi shells. Outer cmd / pwsh wrappers remain unsupported; matching is lexical, not binary attestation.")]
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

/// Derive the `from` for a self-clear outbox message. Mirrors `send`'s sender
/// derivation (send.rs:218+:235): the Root Agent MUST send as `ROOT_AGENT_SENDER`,
/// not the path-derived FQN that `agent_name_from_root` returns for the Root cwd.
///
/// The daemon's anti-spoof gates derive the Root session's name as
/// `ROOT_AGENT_SENDER` (mailbox.rs: the outbox-sender gate via
/// `sender_name_for_session_cwd`, and the token-root gate via
/// `sender_name_for_session_cwd_with_root_flag`). If the CLI stamps anything else,
/// the Root's self-clear is rejected before it ever reaches `handle_self_clear`, so
/// the capability is dead in production. Single source of truth: both `execute` and
/// the daemon e2e tests call this, so reverting it surfaces as a test failure.
pub(crate) fn resolve_self_clear_sender(root: &str) -> String {
    let root_is_root_agent = crate::config::root_agent::is_root_agent_path(root);
    sender_for_root(root, root_is_root_agent)
}

fn self_clear_queued_status(settle_secs: u64) -> String {
    format!(
        "self-handoff-and-clear requested. Phase 1 injects provider-resolved clear text only after this session is continuously idle for {settle_secs}s: /new for an exact-stem direct Pi shell, or /clear for direct Claude/Codex/Gemini-family and Cursor agent shells. Phase 2 then waits a fresh {settle_secs}s of post-clear idle, archives SELF-HANDOFF.md into self-clear/ and injects a prompt naming the exact archived file to resume from. If SELF-FORGET.md was present at queue time, that prompt includes a compact closed-background forgotten summary. Best-effort and NOT guaranteed (a busy session or a daemon restart drops it). If your context is still present later, re-issue."
    )
}

pub fn execute(args: SelfClearArgs) -> i32 {
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
        action: Some(crate::phone::mailbox::SELF_CLEAR_ACTION.to_string()),
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
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
                            "{}",
                            self_clear_queued_status(
                                crate::phone::mailbox::SELF_CLEAR_SETTLE_SECS
                            )
                        ),
                        Some("already_queued") => crate::cli_println!(
                            "self-handoff-and-clear already pending for this session (or a clear/handoff is in \
                             flight); Phase 1 runs after {}s sustained idle, then Phase 2 after a fresh window. \
                             The first queued request owns any forgotten summary; this request does not refresh it. \
                             Best-effort; re-issue later if needed.",
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
        let resp = r#"{"action":"self-handoff-and-clear","status":"queued","session_id":"s","settle_secs":30}"#;
        assert_eq!(interpret_self_clear_response_exit_code(resp), 0);
    }

    #[test]
    fn already_queued_status_returns_zero() {
        let resp = r#"{"action":"self-handoff-and-clear","status":"already_queued","session_id":"s","settle_secs":30}"#;
        assert_eq!(interpret_self_clear_response_exit_code(resp), 0);
    }

    #[test]
    fn unknown_status_returns_two() {
        let resp = r#"{"status":"weird_new_state","action":"self-handoff-and-clear"}"#;
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
        let resp = r#"{"action":"self-handoff-and-clear","session_id":"s"}"#;
        assert_eq!(interpret_self_clear_response_exit_code(resp), 2);
    }

    #[test]
    fn non_string_status_returns_two() {
        let resp = r#"{"status":42,"action":"self-handoff-and-clear"}"#;
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

    // ── clap parse smoke ──

    #[test]
    fn self_clear_args_parse_with_default_timeout() {
        use clap::Parser;
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "self-handoff-and-clear",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--root",
            "anything",
        ])
        .expect("clap should accept self-handoff-and-clear with token + root");
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
            "self-handoff-and-clear",
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

    /// #626 rename: the old `self-clear` subcommand name no longer exists. clap must
    /// REJECT it (the variant is renamed via `#[command(name = "self-handoff-and-clear")]`),
    /// so a stale invocation fails loudly rather than silently doing nothing.
    #[test]
    fn old_self_clear_subcommand_name_is_rejected() {
        use clap::Parser;
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "self-clear",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--root",
            "anything",
        ]);
        assert!(
            parsed.is_err(),
            "the old `self-clear` name must be rejected after the #626 rename"
        );
    }

    #[test]
    fn self_clear_help_documents_provider_resolved_clear() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let mut subcommand = cmd
            .get_subcommands()
            .find(|cmd| cmd.get_name() == "self-handoff-and-clear")
            .expect("self-handoff-and-clear subcommand")
            .clone();
        let help = subcommand.render_long_help().to_string();

        assert!(help.contains("provider-resolved clear text"), "{help}");
        assert!(help.contains("/new"), "{help}");
        assert!(help.contains("/clear"), "{help}");
        assert!(help.contains("exact-stem direct Pi shell"), "{help}");
        assert!(help.contains("continuously idle"), "{help}");
        assert!(help.contains("Outer cmd / pwsh wrappers"), "{help}");
        assert!(!help.contains("then injects /clear."), "{help}");
    }

    #[test]
    fn self_clear_queued_status_documents_provider_resolved_clear() {
        let status = self_clear_queued_status(30);
        assert!(status.contains("provider-resolved clear text"), "{status}");
        assert!(status.contains("/new"), "{status}");
        assert!(status.contains("/clear"), "{status}");
        assert!(status.contains("exact-stem direct Pi shell"), "{status}");
        assert!(status.contains("continuously idle for 30s"), "{status}");
        assert!(!status.contains("Phase 1 injects /clear"), "{status}");
    }

    #[test]
    fn self_clear_help_documents_forgotten_summary_behavior() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let mut subcommand = cmd
            .get_subcommands()
            .find(|cmd| cmd.get_name() == "self-handoff-and-clear")
            .expect("self-handoff-and-clear subcommand")
            .clone();
        let help = subcommand.render_long_help().to_string();

        assert!(help.contains("SELF-FORGET.md"), "{help}");
        assert!(help.contains("240"), "{help}");
        assert!(help.contains("closed background"), "{help}");
        assert!(help.contains("SELF-HANDOFF.md"), "{help}");
        // #749 - the help documents the pre-inject handoff archive and its exact destination.
        assert!(
            help.contains("self-clear/<timestamp>_SELF-HANDOFF.md"),
            "{help}"
        );
    }

    // ── #617 HIGH-1: Root self-clear sender derivation (the actual fix) ──

    /// The Root Agent's self-clear must send as the reserved `ROOT_AGENT_SENDER`,
    /// NOT the path-derived FQN. This is the discriminating gate for the fix: on the
    /// pre-fix code (`agent_name_from_root` on the Root cwd) this returns a path-like
    /// FQN and the assertion fails; with `sender_for_root` it returns the reserved
    /// constant. `is_root_agent_path` recognizes the canonical `root_agent_dir()` by
    /// string-equality even when the directory is absent, so no filesystem setup is
    /// needed.
    #[test]
    fn self_clear_sender_for_root_agent_uses_reserved_constant() {
        let root = crate::config::root_agent::root_agent_dir().expect("resolve root agent dir");
        assert_eq!(
            resolve_self_clear_sender(&root),
            crate::config::root_agent::ROOT_AGENT_SENDER,
            "Root self-clear must send as ROOT_AGENT_SENDER so the daemon anti-spoof accepts it"
        );
    }

    /// A non-Root agent's self-clear is unaffected by the fix: it still resolves to
    /// the path FQN, never the reserved Root URI. Guards against over-reach.
    #[test]
    fn self_clear_sender_for_non_root_is_path_fqn() {
        let sender = resolve_self_clear_sender("C:/proj/.ac/wg-1-team/__agent_dev-rust");
        assert_ne!(sender, crate::config::root_agent::ROOT_AGENT_SENDER);
        assert_eq!(sender, "proj:wg-1-team/dev-rust");
    }
}
