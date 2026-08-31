use clap::Args;
use std::path::PathBuf;
use uuid::Uuid;

use crate::phone::types::OutboxMessage;

/// Pure: map the daemon's self-handoff-and-restart response body to a CLI exit code.
/// 0 = queued | already_queued. 2 = unparseable / missing / non-string / unknown status
/// (daemon spoke incoherently). Byte-equivalent to
/// `self_clear::interpret_self_clear_response_exit_code`.
fn interpret_self_restart_response_exit_code(content: &str) -> i32 {
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
Hands off, then RESPAWNS the caller's OWN coding-agent process on the same coding agent and the \
same profile letter it is already running, and resumes from the handoff file in the NEW session. \
Two deferred phases:\n\n\
  Phase 1 (respawn): waits until this session is continuously idle for 30s, then restarts this \
session with the same coding agent and profile letter it is running now. A genuinely new process, \
not an in-process clear.\n\
  Phase 2 (handoff): after the respawn, waits a FRESH 30s of sustained idle on the NEW session, \
archives SELF-HANDOFF.md -> self-clear/<timestamp>_SELF-HANDOFF.md in your root, then injects a \
prompt naming that exact archived path to resume from. If the archive rename fails, the prompt \
points at SELF-HANDOFF.md still in your root.\n\n\
BEFORE invoking, write SELF-HANDOFF.md in your own root with the notes you need to resume \
(EXCLUDING anything already recorded in SELF-FORGET.md). If SELF-HANDOFF.md is missing, the \
command refuses (respawning with nothing to resume from would wipe your context).\n\n\
On invocation the command archives SELF-FORGET.md -> self-clear/<timestamp>_SELF-FORGET.md in \
your root (no-op if absent), so your next cycle starts with a fresh SELF-FORGET.md. The later \
resume prompt may include a sanitized compact summary of it, max 240 chars, only as closed \
background.\n\n\
RECIPE: the respawn is rebuilt from your CURRENT configuration for the coding agent and profile \
letter this session is running. It pins that recipe, not a frozen command line: if that coding \
agent's configured command is edited between launch and restart, the new process runs the edited \
command.\n\n\
SELECTION: this command does not change your Selection-UI coding-agent or profile assignment. \
Like any restart, the respawn does refresh this agent root's tooling.lastCodingAgent and \
tooling.codingAgents bookkeeping.\n\n\
SCOPE: any session that owns a token and runs a configured coding agent, including Room replica \
agents, origin Agent Matrix agents, and the Root Agent. A session with no configured coding-agent \
identity is refused: there is no recipe to respawn.\n\n\
IDENTITY: the session is resolved from --token (find_by_token). You can only restart the session \
that owns the token you present.\n\n\
DELIVERY CAVEAT: when the respawned shell is not a direct coding-agent CLI (an outer cmd or pwsh \
wrapper, for example), the resume prompt is written into the new session WITHOUT an appended \
Enter, so it is delivered but not submitted; a human or the agent must press Enter to act on it.\n\n\
BEST-EFFORT: neither phase is guaranteed. A perpetually busy session that never reaches 30s \
sustained idle, or a daemon restart mid-cycle, drops the remainder (a greppable warn line is \
logged). Re-issue if your context is still present later.")]
pub struct SelfRestartArgs {
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

fn self_restart_queued_status(settle_secs: u64) -> String {
    format!(
        "self-handoff-and-restart requested. Phase 1 respawns this session only after it is continuously idle for {settle_secs}s, on the same configured coding agent and the same profile letter it is running now; your Selection-UI coding-agent and profile assignment are not changed. Phase 2 then waits a fresh {settle_secs}s of sustained idle on the NEW session, archives SELF-HANDOFF.md into self-clear/ and injects a prompt naming the exact archived file to resume from. If SELF-FORGET.md was present at queue time, that prompt includes a compact closed-background forgotten summary. Best-effort and NOT guaranteed (a busy session or a daemon restart drops it). If your context is still present later, re-issue."
    )
}

/// #1632 - build the self-handoff-and-restart outbox message, INCLUDING its `from`.
///
/// This is a function rather than four inline lines inside `execute` on purpose. The landed
/// `self_clear.rs` builds its `OutboxMessage` inline (`self_clear.rs:105-134`), which is why no
/// test in the tree asserts that `execute` stamps the right sender; extracting the builder makes
/// the CLI-to-helper link a pure function a test can call, which is what T26 does.
///
/// `resolve_self_clear_sender` is reused verbatim, and that reuse is load-bearing: it is what
/// makes the Root Agent send as `ROOT_AGENT_SENDER` rather than the path-derived FQN, which is
/// what the daemon's two anti-spoof gates accept. Stamp anything else and the Root Agent's
/// restart is rejected before it ever reaches `handle_self_restart`.
pub(crate) fn build_self_restart_outbox_message(
    root: &str,
    token: Option<String>,
    msg_id: String,
    request_id: String,
) -> OutboxMessage {
    OutboxMessage {
        id: msg_id,
        token,
        from: super::self_clear::resolve_self_clear_sender(root),
        to: String::new(), // empty skips the `if !msg.to.is_empty()` resolution guard in
        // process_message; self-handoff-and-restart resolves by token and never reads `to`.
        body: String::new(),
        mode: String::new(),
        get_output: false,
        request_id: Some(request_id),
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
        action: Some(crate::phone::mailbox::SELF_RESTART_ACTION.to_string()),
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    }
}

pub fn execute(args: SelfRestartArgs) -> i32 {
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

    let ac_dir = PathBuf::from(&root).join(crate::config::agent_local_dir_name());

    let msg_id = Uuid::new_v4().to_string();
    let request_id = Uuid::new_v4().to_string();

    let message =
        build_self_restart_outbox_message(&root, args.token, msg_id.clone(), request_id.clone());

    // Outbox selection mirrors self_clear::execute (is_root -> app outbox else agent outbox).
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

    // Poll delivered/ | rejected/ (queue confirmation, 30s) - same loop as self_clear::execute.
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
                "Error: self-handoff-and-restart rejected - {}",
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

    // Poll response (queue ack). The daemon writes it immediately after queuing,
    // NOT after the respawn runs - so this is fast.
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
                    // Honest, conditional wording: the deferred respawn is best-effort, NOT a
                    // near-certainty. The "30" is single-sourced from the daemon const.
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
                            self_restart_queued_status(
                                crate::phone::mailbox::SELF_CLEAR_SETTLE_SECS
                            )
                        ),
                        Some("already_queued") => crate::cli_println!(
                            "self-handoff-and-restart already pending for this session (or a clear/switch/restart is \
                             in flight); Phase 1 runs after {}s sustained idle, then Phase 2 after a fresh window. \
                             The first queued request owns any forgotten summary; this request does not refresh it. \
                             Best-effort; re-issue later if needed.",
                            crate::phone::mailbox::SELF_CLEAR_SETTLE_SECS
                        ),
                        _ => {}
                    }
                    return interpret_self_restart_response_exit_code(&content);
                }
                Err(e) => {
                    eprintln!("Error: failed to read response: {}", e);
                    return 1;
                }
            }
        }
        if resp_start.elapsed() >= resp_timeout {
            eprintln!(
                "Error: self-handoff-and-restart delivered but no queue ack within {}s (request {})",
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

    // ── T15: interpret_self_restart_response_exit_code (mirrors self_clear's table) ──

    #[test]
    fn queued_status_returns_zero() {
        let resp = r#"{"action":"self-handoff-and-restart","status":"queued","session_id":"s","settle_secs":30}"#;
        assert_eq!(interpret_self_restart_response_exit_code(resp), 0);
    }

    #[test]
    fn already_queued_status_returns_zero() {
        let resp = r#"{"action":"self-handoff-and-restart","status":"already_queued","session_id":"s","settle_secs":30}"#;
        assert_eq!(interpret_self_restart_response_exit_code(resp), 0);
    }

    #[test]
    fn unknown_status_returns_two() {
        let resp = r#"{"status":"weird_new_state","action":"self-handoff-and-restart"}"#;
        assert_eq!(interpret_self_restart_response_exit_code(resp), 2);
    }

    #[test]
    fn unparseable_json_returns_two() {
        assert_eq!(interpret_self_restart_response_exit_code("not json"), 2);
        assert_eq!(interpret_self_restart_response_exit_code(""), 2);
        assert_eq!(interpret_self_restart_response_exit_code("{partial"), 2);
    }

    #[test]
    fn missing_status_field_returns_two() {
        let resp = r#"{"action":"self-handoff-and-restart","session_id":"s"}"#;
        assert_eq!(interpret_self_restart_response_exit_code(resp), 2);
    }

    #[test]
    fn non_string_status_returns_two() {
        let resp = r#"{"status":42,"action":"self-handoff-and-restart"}"#;
        assert_eq!(interpret_self_restart_response_exit_code(resp), 2);
    }

    /// Valid JSON that is not an object (array, scalar, null) still fails the
    /// `.get("status")` lookup and must return exit 2.
    #[test]
    fn non_object_json_returns_two() {
        assert_eq!(interpret_self_restart_response_exit_code("null"), 2);
        assert_eq!(interpret_self_restart_response_exit_code("[1,2,3]"), 2);
        assert_eq!(interpret_self_restart_response_exit_code("\"queued\""), 2);
        assert_eq!(interpret_self_restart_response_exit_code("42"), 2);
        assert_eq!(interpret_self_restart_response_exit_code("true"), 2);
    }

    // ── T16: clap parse smoke ──

    #[test]
    fn self_restart_args_parse_with_default_timeout() {
        use clap::Parser;
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "self-handoff-and-restart",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--root",
            "anything",
        ])
        .expect("clap should accept self-handoff-and-restart with token + root");
        let cmd = parsed.command.expect("subcommand present");
        match cmd {
            crate::cli::Commands::SelfRestart(args) => {
                assert_eq!(args.timeout, 15, "--timeout must default to 15");
                assert_eq!(
                    args.token.as_deref(),
                    Some("11111111-1111-1111-1111-111111111111")
                );
                assert_eq!(args.root.as_deref(), Some("anything"));
            }
            _ => panic!("expected SelfRestart subcommand"),
        }
    }

    #[test]
    fn self_restart_args_parse_explicit_timeout() {
        use clap::Parser;
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "self-handoff-and-restart",
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
            crate::cli::Commands::SelfRestart(args) => assert_eq!(args.timeout, 42),
            _ => panic!("expected SelfRestart subcommand"),
        }
    }

    /// The verb ships under its full name only. A bare `self-restart` must be REJECTED so a
    /// guessed invocation fails loudly rather than silently doing nothing.
    #[test]
    fn bare_self_restart_subcommand_name_is_rejected() {
        use clap::Parser;
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "self-restart",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--root",
            "anything",
        ]);
        assert!(
            parsed.is_err(),
            "the bare `self-restart` name must be rejected; the verb is `self-handoff-and-restart`"
        );
    }

    // ── T17: help content, including the two shipped-string prohibitions ──

    /// The positive needles are checked against a whitespace-normalized rendering, because
    /// clap re-wraps `after_help` to the render width and a wrap landing inside a multi-word
    /// needle would make the assertion fail for a reason that has nothing to do with the
    /// prose. The negative needles are checked against BOTH, since a prohibited substring
    /// must be absent however the text is folded.
    #[test]
    fn self_restart_help_documents_scope_recipe_and_no_enter_caveat() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let mut subcommand = cmd
            .get_subcommands()
            .find(|cmd| cmd.get_name() == "self-handoff-and-restart")
            .expect("self-handoff-and-restart subcommand")
            .clone();
        let raw = subcommand.render_long_help().to_string();
        let help = raw.split_whitespace().collect::<Vec<_>>().join(" ");

        assert!(help.contains("SELF-HANDOFF.md"), "{raw}");
        assert!(
            help.contains("self-clear/<timestamp>_SELF-HANDOFF.md"),
            "{raw}"
        );
        assert!(help.contains("Root Agent"), "{raw}");
        assert!(help.contains("same coding agent"), "{raw}");
        assert!(help.contains("Room replica"), "{raw}");
        assert!(help.contains("continuously idle for 30s"), "{raw}");
        assert!(
            help.contains("does not change your Selection-UI coding-agent or profile assignment"),
            "{raw}"
        );
        // D1's no-Enter caveat: on a shell that is not a direct coding-agent CLI the resume
        // prompt is delivered, not submitted.
        assert!(help.contains("WITHOUT an appended Enter"), "{raw}");
        assert!(help.contains("delivered but not submitted"), "{raw}");

        // Round-2 BL1 / R2: both of these claims are FALSE and must never ship in a
        // user-facing string. Every restart rewrites tooling.lastCodingAgent, and the respawn
        // is rebuilt from current settings rather than replayed from a stored argv.
        for prohibited in ["no settings", "writes no settings", "same parameters"] {
            assert!(!raw.contains(prohibited), "{prohibited}: {raw}");
            assert!(!help.contains(prohibited), "{prohibited}: {help}");
        }
        // The repository writes prose without em-dashes, and this help says "Room replica".
        assert!(!raw.contains('\u{2014}'), "{raw}");
        for needle in ["workgroup", "Workgroup", "WORKGROUP"] {
            assert!(!raw.contains(needle), "{needle}: {raw}");
        }
    }

    #[test]
    fn self_restart_queued_status_is_honest_about_the_recipe_and_the_selection() {
        let status = self_restart_queued_status(30);
        assert!(status.contains("continuously idle for 30s"), "{status}");
        assert!(
            status.contains("same configured coding agent and the same profile letter"),
            "{status}"
        );
        assert!(!status.contains("no settings"), "{status}");
        assert!(!status.contains("same parameters"), "{status}");
        assert!(!status.contains('\u{2014}'), "{status}");
    }

    // ── T18: sender derivation, asserted from the restart module ──

    /// The Root Agent's self-handoff-and-restart must send as the reserved
    /// `ROOT_AGENT_SENDER`, NOT the path-derived FQN. Asserted from THIS module so a future
    /// divergence on the restart path surfaces here. `is_root_agent_path` recognizes the
    /// canonical `root_agent_dir()` by string equality even when the directory is absent, so
    /// no filesystem setup is needed.
    ///
    /// T18 alone is NOT sufficient for the Root Agent scope: it asserts the helper in
    /// isolation and nothing in it proves the restart path calls it. T26 below closes the
    /// builder-to-helper link; the two Root `process_message` e2es in `phone/mailbox.rs` close
    /// the "a correctly stamped message survives the daemon's anti-spoof gates" half.
    #[test]
    fn self_restart_sender_for_root_agent_uses_reserved_constant() {
        let root = crate::config::root_agent::root_agent_dir().expect("resolve root agent dir");
        assert_eq!(
            crate::cli::self_clear::resolve_self_clear_sender(&root),
            crate::config::root_agent::ROOT_AGENT_SENDER,
            "Root self-handoff-and-restart must send as ROOT_AGENT_SENDER so the daemon anti-spoof accepts it"
        );
    }

    // ── T26: the builder stamps the canonical sender, the action, and an empty `to` ──

    #[test]
    fn build_self_restart_outbox_message_stamps_canonical_root_sender() {
        let root = crate::config::root_agent::root_agent_dir().expect("resolve root agent dir");
        let root_msg =
            build_self_restart_outbox_message(&root, Some("tok".into()), "m".into(), "r".into());
        assert_eq!(
            root_msg.from,
            crate::config::root_agent::ROOT_AGENT_SENDER,
            "the BUILDER itself must stamp ROOT_AGENT_SENDER for the Root cwd"
        );
        assert_eq!(
            root_msg.action.as_deref(),
            Some(crate::phone::mailbox::SELF_RESTART_ACTION)
        );
        assert!(root_msg.to.is_empty());

        // Over-reach guard: a builder that returned the reserved constant unconditionally
        // would pass the row above. A non-Root agent must still get the path FQN.
        let replica_msg = build_self_restart_outbox_message(
            "C:/proj/.ac/wg-1-team/__agent_dev-rust",
            None,
            "m2".into(),
            "r2".into(),
        );
        assert_eq!(replica_msg.from, "proj:wg-1-team/dev-rust");
        assert_ne!(
            replica_msg.from,
            crate::config::root_agent::ROOT_AGENT_SENDER
        );
        assert_eq!(
            replica_msg.action.as_deref(),
            Some(crate::phone::mailbox::SELF_RESTART_ACTION)
        );
        assert!(replica_msg.to.is_empty());
    }
}
