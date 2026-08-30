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
then resumes from the handoff file.\n\n\
BEFORE invoking, write SELF-HANDOFF.md in your own root with the notes you need to resume, EXCLUDING \
anything already recorded in SELF-FORGET.md. If SELF-HANDOFF.md is missing, the command refuses.\n\n\
On invocation the daemon captures a sanitized compact forgotten summary from the current replica's \
SELF-FORGET.md, max 240 chars, then archives SELF-FORGET.md -> self-clear/<timestamp>_SELF-FORGET.md \
(no-op if absent). The later resume prompt may include that summary only as closed background, not \
instructions and not work to resume. The handoff file remains the active resume source: when the \
resume prompt fires, SELF-HANDOFF.md is first archived -> self-clear/<timestamp>_SELF-HANDOFF.md and \
the prompt names that exact archived path (if the rename fails, the prompt points at SELF-HANDOFF.md \
still in your root).\n\n\
--coding-agent takes the configured coding-agent entry id from settings, not the backend kind \
or AC peer name. Use --list-coding-agents to print valid ids and profile letters without \
sending a switch request. If the id is unknown, the daemon rejection lists configured ids.\n\n\
Omitting both --coding-agent and --profile is allowed. It hard-resets the live running recipe \
by respawning the same coding agent/profile fresh and then injecting the handoff prompt.\n\n\
SCOPE: Room replicas only (__agent_* under a room-* or legacy wg-* room). Root Agent and origin matrix agents \
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
    if settings.agents.is_empty() {
        return "No configured coding agents found.".to_string();
    }

    let mut lines = vec![
        "Configured coding agents for self-handoff-and-switch:".to_string(),
        "Use AgentConfig.id exactly with --coding-agent. Use --profile A-Z, or omit --profile to keep the caller's current effective profile.".to_string(),
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
            "  fallback: other A-Z letters fall back to the nearest lower profile listed above, ending at A."
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
                             session, archives SELF-HANDOFF.md into self-clear/ and injects a prompt naming \
                             the exact archived file to resume from. If SELF-FORGET.md was present at queue \
                             time, that prompt includes a compact closed-background forgotten summary. \
                             Best-effort and not guaranteed.",
                            crate::phone::mailbox::SELF_CLEAR_SETTLE_SECS
                        ),
                        Some("already_queued") => crate::cli_println!(
                            "A self context operation is already pending for this session. The first queued request owns any forgotten summary; this request does not refresh it. Best-effort; re-issue later if needed."
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
                assert!(!args.list_coding_agents);
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
                assert!(!args.list_coding_agents);
            }
            _ => panic!("expected SelfSwitch subcommand"),
        }
    }

    #[test]
    fn self_switch_list_coding_agents_parses_without_token_or_root() {
        use clap::Parser;
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "self-handoff-and-switch",
            "--list-coding-agents",
        ])
        .expect("discovery should not require token or root at clap parse time");
        let cmd = parsed.command.expect("subcommand present");
        match cmd {
            crate::cli::Commands::SelfSwitch(args) => {
                assert!(args.list_coding_agents);
                assert_eq!(args.token, None);
                assert_eq!(args.root, None);
                assert_eq!(args.coding_agent, None);
                assert_eq!(args.profile, None);
            }
            _ => panic!("expected SelfSwitch subcommand"),
        }
    }

    #[test]
    fn self_switch_help_documents_forgotten_summary_behavior() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let mut subcommand = cmd
            .get_subcommands()
            .find(|cmd| cmd.get_name() == "self-handoff-and-switch")
            .expect("self-handoff-and-switch subcommand")
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

    fn agent(id: &str, label: &str, command: &str) -> crate::config::settings::AgentConfig {
        crate::config::settings::AgentConfig {
            id: id.to_string(),
            label: label.to_string(),
            command: command.to_string(),
            color: "#000000".to_string(),
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            backend: Default::default(),
        }
    }

    fn enabled_cell(command: &str) -> crate::config::settings::ProfileCellConfig {
        crate::config::settings::ProfileCellConfig {
            enabled: true,
            command: command.to_string(),
            env: std::collections::BTreeMap::new(),
            notes: String::new(),
        }
    }

    fn disabled_cell(command: &str) -> crate::config::settings::ProfileCellConfig {
        crate::config::settings::ProfileCellConfig {
            enabled: false,
            command: command.to_string(),
            env: std::collections::BTreeMap::new(),
            notes: String::new(),
        }
    }

    #[test]
    fn discovery_lists_exact_ids_for_duplicate_backend_commands() {
        let mut settings = crate::config::settings::AppSettings {
            agents: vec![
                agent("codex-main", "Codex Main", "codex --model gpt-5"),
                agent("codex-review", "Codex Review", "codex --model gpt-5"),
            ],
            ..crate::config::settings::AppSettings::default()
        };
        settings.coding_agent_profiles.profiles_by_agent.insert(
            "codex-main".to_string(),
            std::collections::BTreeMap::from([
                ("A".to_string(), enabled_cell("")),
                ("B".to_string(), enabled_cell("--profile-b")),
            ]),
        );
        settings.coding_agent_profiles.profiles_by_agent.insert(
            "codex-review".to_string(),
            std::collections::BTreeMap::from([
                ("A".to_string(), enabled_cell("")),
                ("C".to_string(), enabled_cell("--profile-c")),
            ]),
        );

        let rendered = render_coding_agent_discovery(&settings);

        assert!(rendered.contains("- codex-main"));
        assert!(rendered.contains("- codex-review"));
        assert!(rendered.contains("self-handoff-and-switch --coding-agent=codex-main --profile B"));
        assert!(
            rendered.contains("self-handoff-and-switch --coding-agent=codex-review --profile C")
        );
        assert!(!rendered.contains("self-handoff-and-switch --coding-agent codex --profile"));
    }

    #[test]
    fn discovery_reports_direct_profiles_and_fallback_rule() {
        let mut settings = crate::config::settings::AppSettings {
            agents: vec![agent("codex-main", "Codex Main", "codex")],
            ..crate::config::settings::AppSettings::default()
        };
        settings.coding_agent_profiles.profiles_by_agent.insert(
            "codex-main".to_string(),
            std::collections::BTreeMap::from([
                ("A".to_string(), enabled_cell("")),
                ("B".to_string(), disabled_cell("--disabled")),
                ("D".to_string(), enabled_cell("--profile-d")),
            ]),
        );

        let rendered = render_coding_agent_discovery(&settings);

        assert!(rendered.contains("profiles without fallback: A, D"));
        assert!(
            rendered.contains("other A-Z letters fall back to the nearest lower profile listed")
        );
        assert!(!rendered.contains("self-handoff-and-switch --coding-agent=codex-main --profile B"));
    }

    #[test]
    fn discovery_shell_quotes_copy_ids_with_spaces_and_metacharacters() {
        let settings = crate::config::settings::AppSettings {
            agents: vec![agent("codex main; Remove-Item *", "Codex Main", "codex")],
            ..crate::config::settings::AppSettings::default()
        };

        let rendered = render_coding_agent_discovery(&settings);

        assert!(rendered.contains(
            "self-handoff-and-switch --coding-agent='codex main; Remove-Item *' --profile A"
        ));
    }
}
