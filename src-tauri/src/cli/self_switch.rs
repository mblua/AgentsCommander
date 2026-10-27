use clap::Args;
use std::path::PathBuf;
use uuid::Uuid;

use crate::filesystem::{directories, documents};
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
#[command(
    after_help = "Hands off, switches the CALLER'S OWN session coding agent and/or profile, \
respawns it fresh, then resumes from the handoff file.\n\nBEFORE invoking, write SELF-HANDOFF.md \
in your own root with the notes you need to resume, EXCLUDING anything already recorded in \
SELF-FORGET.md. If SELF-HANDOFF.md is missing, the command refuses.\n\nOn invocation the daemon \
captures a sanitized compact forgotten summary from the current replica's SELF-FORGET.md, max 240 \
chars, then archives SELF-FORGET.md -> self-clear/<timestamp>_SELF-FORGET.md (no-op if absent). \
The later resume prompt may include that summary only as closed background, not instructions and \
not work to resume. The handoff file remains the active resume source: when the resume prompt \
fires, SELF-HANDOFF.md is first archived -> self-clear/<timestamp>_SELF-HANDOFF.md and the prompt \
names that exact archived path (if the rename fails, the prompt points at SELF-HANDOFF.md still \
in your root).\n\n--coding-agent takes the configured coding-agent entry id from settings, not \
the backend kind or AC peer name. Use --list-coding-agents to print valid ids and profile letters \
without sending a switch request. If the id is unknown, the daemon rejection lists configured \
ids.\n\nOmitting both --coding-agent and --profile is allowed. It hard-resets the live running \
recipe by respawning the same coding agent/profile fresh and then injecting the handoff \
prompt.\n\nSCOPE: Room replicas only (__agent_* under a room-* or legacy wg-* room). Root Agent \
and origin matrix agents are rejected in v1."
)]
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

    /// List configured coding-agent ids and profile letters for this command, then exit.
    #[arg(long = "list-coding-agents")]
    pub list_coding_agents: bool,

    /// Seconds to wait for the daemon's queue acknowledgement (default 15).
    #[arg(long, default_value = "15")]
    pub timeout: u64,
}

fn all_profile_letters() -> impl Iterator<Item = String> {
    (b'A'..=b'Z').map(|byte| (byte as char).to_string())
}

fn direct_profile_letters(
    settings: &crate::config::settings::AppSettings,
    coding_agent_id: &str,
) -> Vec<String> {
    all_profile_letters()
        .filter(|letter| {
            let resolution = crate::config::coding_agent_profiles::resolve_profile(
                settings,
                crate::config::coding_agent_profiles::ProfileResolutionRequest {
                    coding_agent_id,
                    launch_path: None,
                    agent_matrix_name: None,
                    requested_profile: Some(letter),
                    requested_profile_authoritative: false,
                },
            );
            resolution.effective_profile == *letter
        })
        .collect()
}

fn quote_coding_agent_id_for_copy(id: &str) -> String {
    if !id.is_empty()
        && id.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/' | '\\' | '@')
        })
    {
        return id.to_string();
    }

    single_quote_arg_for_copy(id)
}

#[cfg(windows)]
fn single_quote_arg_for_copy(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "''"))
}

#[cfg(not(windows))]
fn single_quote_arg_for_copy(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "'\\''"))
}

fn render_coding_agent_discovery(settings: &crate::config::settings::AppSettings) -> String {
    // #2716 (B3, option C): an unreadable agents layer is not "none configured".
    if let Some(path) = settings.agents_layer.unreadable_path() {
        return crate::config::settings::agents_unreadable_cli_error(path);
    }
    if settings.agents.is_empty() {
        return "No configured coding agents found.".to_string();
    }

    let mut lines = vec![
        "Configured coding agents for self-handoff-and-switch:".to_string(),
        "Use AgentConfig.id exactly with --coding-agent. Use --profile A-Z, or omit --profile to \
        keep the caller's current effective profile."
            .to_string(),
        String::new(),
    ];

    for agent in &settings.agents {
        let direct_profiles = direct_profile_letters(settings, &agent.id);
        let direct_profiles = if direct_profiles.is_empty() {
            "A".to_string()
        } else {
            direct_profiles.join(", ")
        };

        lines.push(format!("- {}", agent.id));
        if !agent.label.trim().is_empty() {
            lines.push(format!("  label: {}", agent.label));
        }
        lines.push(format!("  command: {}", agent.command));
        lines.push(format!("  profiles without fallback: {}", direct_profiles));
        lines.push(
            "  fallback: other A-Z letters fall back to the nearest lower profile listed above, \
            ending at A."
                .to_string(),
        );
        lines.push("  copy:".to_string());
        let agent_id_for_copy = quote_coding_agent_id_for_copy(&agent.id);
        for profile in direct_profiles.split(", ") {
            lines.push(format!(
                "    self-handoff-and-switch --coding-agent={} --profile {}",
                agent_id_for_copy, profile
            ));
        }
        lines.push(String::new());
    }

    lines.pop();
    lines.join("\n")
}

fn print_coding_agent_discovery() {
    let settings = crate::config::settings::load_settings_for_cli();
    crate::cli_println!("{}", render_coding_agent_discovery(&settings));
}

pub fn execute(args: SelfSwitchArgs) -> i32 {
    if args.list_coding_agents {
        print_coding_agent_discovery();
        return 0;
    }

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
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: None,
        action: Some(crate::phone::mailbox::SELF_SWITCH_ACTION.to_string()),
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: args.coding_agent,
        switch_profile,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    };

    let outbox_dir = if is_root {
        let app_outbox = crate::config::config_dir()
            .map(|d| d.join("app-outbox-path.txt"))
            .and_then(|p| documents::read_cli_outbox_text(&p).ok())
            .map(|s| PathBuf::from(s.trim()));
        match app_outbox {
            Some(p) if directories::cli_agent_root_is_dir(&p) => p,
            _ => ac_dir.join("outbox"),
        }
    } else {
        ac_dir.join("outbox")
    };
    if let Err(e) = directories::ensure_cli_outbox(&outbox_dir) {
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
    if let Err(e) = documents::write_cli_outbox_message(&outbox_path, json.as_bytes()) {
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
        if directories::cli_outbox_artifact_exists(&delivered_path) {
            break;
        }
        if directories::cli_outbox_artifact_exists(&rejected_reason_path) {
            let reason = documents::read_cli_outbox_text(&rejected_reason_path)
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
        if directories::cli_outbox_artifact_exists(&response_path) {
            match documents::read_cli_outbox_text(&response_path) {
                Ok(content) => {
                    crate::cli_println!("{}", content);
                    match serde_json::from_str::<serde_json::Value>(&content)
                        .ok()
                        .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(String::from))
                        .as_deref()
                    {
                        Some("queued") => crate::cli_println!(
                            "self-handoff-and-switch requested. Phase 1 respawns only after this \
                            session is continuously idle for {0}s; Phase 2 then waits a fresh \
                            {0}s of idle in the new session, archives SELF-HANDOFF.md into \
                            self-clear/ and injects a prompt naming the exact archived file to \
                            resume from. If SELF-FORGET.md was present at queue time, that prompt \
                            includes a compact closed-background forgotten summary. Best-effort \
                            and not guaranteed.",
                            crate::phone::mailbox::SELF_CLEAR_SETTLE_SECS
                        ),
                        Some("already_queued") => crate::cli_println!(
                            "A self context operation is already pending for this session. The \
                            first queued request owns any forgotten summary; this request does \
                            not refresh it. Best-effort; re-issue later if needed."
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
mod tests;
