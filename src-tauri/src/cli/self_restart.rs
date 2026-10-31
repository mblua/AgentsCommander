use clap::Args;
use std::path::PathBuf;
use uuid::Uuid;

use crate::filesystem::{directories, documents};
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
BEFORE invoking, write SELF-HANDOFF.md in your own root following the self-handoff skill. \
If SELF-HANDOFF.md is missing, the \
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
Enter, so it is delivered but not submitted; a human or the agent must press Enter to act on \
it.\n\n\
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
        "self-handoff-and-restart requested. Phase 1 respawns this session only after it is \
        continuously idle for {settle_secs}s, on the same configured coding agent and the same \
        profile letter it is running now; your Selection-UI coding-agent and profile \
        assignment are not changed. Phase 2 then waits a fresh {settle_secs}s of sustained \
        idle on the NEW session, archives SELF-HANDOFF.md into self-clear/ and injects a \
        prompt naming the exact archived file to resume from. If SELF-FORGET.md was present at \
        queue time, that prompt includes a compact closed-background forgotten summary. \
        Best-effort and NOT guaranteed (a busy session or a daemon restart drops it). If your \
        context is still present later, re-issue."
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
    let outbox_dir = crate::filesystem::app_outbox_pointer::directory(
        is_root.then(crate::config::config_dir).flatten().as_deref(),
        &ac_dir,
    );
    if let Err(error) =
        super::outbox_delivery::deliver(&outbox_dir, &message, "self-handoff-and-restart")
    {
        eprintln!("Error: {}", error);
        return 1;
    }

    // Poll response (queue ack). The daemon writes it immediately after queuing,
    // NOT after the respawn runs - so this is fast.
    let responses_dir = ac_dir.join("responses");
    let response_path = responses_dir.join(format!("{}.json", request_id));
    let resp_timeout = std::time::Duration::from_secs(args.timeout);
    let resp_poll = std::time::Duration::from_millis(250);
    let resp_start = std::time::Instant::now();
    loop {
        if directories::cli_outbox_artifact_exists(&response_path) {
            match documents::read_cli_outbox_text(&response_path) {
                Ok(content) => {
                    crate::cli_println!("{}", content);
                    // Honest, conditional wording: the deferred respawn is best-effort, NOT a
                    // near-certainty. The "30" is single-sourced from the daemon const.
                    match serde_json::from_str::<serde_json::Value>(&content)
                        .ok()
                        .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(String::from))
                        .as_deref()
                    {
                        Some("queued") => crate::cli_println!(
                            "{}",
                            self_restart_queued_status(
                                crate::phone::mailbox::SELF_CLEAR_SETTLE_SECS
                            )
                        ),
                        Some("already_queued") => crate::cli_println!(
                            "self-handoff-and-restart already pending for this session (or a \
                            clear/switch/restart is \
                             in flight); Phase 1 runs after {}s sustained idle, then Phase 2 \
                             after a fresh window. \
                             The first queued request owns any forgotten summary; this request \
                             does not refresh it. \
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
                "Error: self-handoff-and-restart delivered but no queue ack within {}s \
                (request {})",
                args.timeout, request_id
            );
            return 2;
        }
        std::thread::sleep(resp_poll);
    }
}

#[cfg(test)]
mod tests;
