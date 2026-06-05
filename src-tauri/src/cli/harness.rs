use clap::Args;
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::process::Command;

const MAX_LOGGED_COMMAND_LEN: usize = 512;

#[derive(Args, Debug)]
#[command(
    about = "Execute commands through the AgentsCommander policy harness",
    after_help = "PHASE 1 (Obedient Harness): This command currently routes execution through AgentsCommander and logs actions, but does not provide strong enforcement or sandboxing. Direct shell execution by agents remains possible.\nNote: --raw-command execution relies on the platform shell and policy matching is best-effort."
)]
pub struct HarnessArgs {
    /// Explain the policy decision without executing the command
    #[arg(long)]
    pub explain: bool,

    /// Evaluate policy and simulate execution without modifying state
    #[arg(long)]
    pub dry_run: bool,

    /// Command to execute, separated by -- (e.g., harness -- git push)
    #[arg(last = true, conflicts_with = "raw_command")]
    pub command: Vec<String>,

    /// Optional literal string command for Windows ergonomics. Executed via the platform shell.
    #[arg(long, conflicts_with = "command")]
    pub raw_command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PolicyDecision {
    Allow,
    Warn(Vec<String>),
    Deny(Vec<String>),
}

#[derive(Debug, Clone)]
pub(crate) struct PolicyInput {
    pub raw_display: String,
    pub argv: Vec<String>,
    pub raw_shell: bool,
}

#[derive(Debug)]
enum ExecutionMode {
    Argv(Vec<String>),
    Raw(String),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessLogEntry {
    timestamp: String,
    identity: String,
    identity_note: &'static str,
    raw_shell: bool,
    command: String,
    command_truncated: bool,
    argv_count: usize,
    command_hash: u64,
    decision: String,
    reasons: Vec<String>,
    mode: String,
    execution_result: String,
    exit_code: Option<i32>,
}

pub fn execute(args: HarnessArgs) -> i32 {
    let mode = match parse_mode(&args) {
        Ok(mode) => mode,
        Err(err) => {
            eprintln!("Error: {}", err);
            return 1;
        }
    };

    let input = match &mode {
        ExecutionMode::Argv(argv) => PolicyInput {
            raw_display: display_argv(argv),
            argv: argv.clone(),
            raw_shell: false,
        },
        ExecutionMode::Raw(raw) => PolicyInput {
            raw_display: raw.clone(),
            argv: Vec::new(),
            raw_shell: true,
        },
    };

    let decision = evaluate_policy(&input);
    print_decision(&decision, &input, args.explain || args.dry_run);

    if let Err(err) = write_audit_log(&input, &decision, "planned", None) {
        eprintln!("Error: failed to write harness audit log: {}", err);
        return 1;
    }

    if matches!(decision, PolicyDecision::Deny(_)) {
        return 1;
    }

    if args.explain || args.dry_run {
        return 0;
    }

    let status = match run_command(&mode) {
        Ok(status) => status,
        Err(err) => {
            let _ = write_audit_log(&input, &decision, "spawn_failed", Some(1));
            eprintln!("Error: failed to execute harness command: {}", err);
            return 1;
        }
    };

    let code = status.code().unwrap_or(1);
    if let Err(err) = write_audit_log(&input, &decision, "completed", Some(code)) {
        eprintln!("Error: failed to write harness completion log: {}", err);
        return 1;
    }
    code
}

fn parse_mode(args: &HarnessArgs) -> Result<ExecutionMode, String> {
    match (&args.raw_command, args.command.is_empty()) {
        (Some(raw), _) if raw.trim().is_empty() => Err("--raw-command cannot be empty".to_string()),
        (Some(raw), _) => Ok(ExecutionMode::Raw(raw.clone())),
        (None, false) => Ok(ExecutionMode::Argv(args.command.clone())),
        (None, true) => Err("provide a command after -- or pass --raw-command".to_string()),
    }
}

fn run_command(mode: &ExecutionMode) -> std::io::Result<std::process::ExitStatus> {
    match mode {
        ExecutionMode::Argv(argv) => {
            let mut command = Command::new(&argv[0]);
            command.args(&argv[1..]);
            command.status()
        }
        ExecutionMode::Raw(raw) => {
            let mut command = platform_shell_command(raw);
            command.status()
        }
    }
}

#[cfg(target_os = "windows")]
fn platform_shell_command(raw: &str) -> Command {
    let mut command = Command::new("cmd.exe");
    command.args(["/C", raw]);
    command
}

#[cfg(not(target_os = "windows"))]
fn platform_shell_command(raw: &str) -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", raw]);
    command
}

pub(crate) fn evaluate_policy(input: &PolicyInput) -> PolicyDecision {
    let mut warnings = Vec::new();
    let mut denials = Vec::new();

    if input.raw_shell {
        evaluate_raw_policy(input, &mut warnings, &mut denials);
    } else {
        evaluate_argv_policy(input, &mut warnings, &mut denials);
    }

    if denials.is_empty() {
        if warnings.is_empty() {
            PolicyDecision::Allow
        } else {
            PolicyDecision::Warn(warnings)
        }
    } else {
        PolicyDecision::Deny(denials)
    }
}

fn evaluate_argv_policy(
    input: &PolicyInput,
    warnings: &mut Vec<String>,
    denials: &mut Vec<String>,
) {
    let tokens: Vec<String> = input.argv.iter().map(|s| normalize_token(s)).collect();
    if tokens.is_empty() {
        return;
    }

    warn_nested_shell(&tokens[0], warnings);
    warn_branch_name(&tokens, warnings);

    let cmd = tokens[0].as_str();
    if cmd == "rm" && has_rm_recursive_force(&tokens) && tokens.iter().any(|t| is_root_target(t)) {
        denials.push("denied destructive root removal".to_string());
    }

    if cmd == "remove-item"
        && has_any_token(&tokens, &["-recurse", "-r"])
        && has_any_token(&tokens, &["-force", "-fo"])
        && tokens.iter().any(|t| is_root_target(t))
    {
        denials.push("denied destructive Remove-Item root removal".to_string());
    }

    if (cmd == "rd" || cmd == "rmdir")
        && has_token(&tokens, "/s")
        && has_token(&tokens, "/q")
        && tokens.iter().any(|t| is_root_target(t))
    {
        denials.push("denied destructive recursive directory removal".to_string());
    }

    if cmd == "del"
        && has_token(&tokens, "/s")
        && has_token(&tokens, "/q")
        && tokens.iter().any(|t| is_root_target(t))
    {
        denials.push("denied destructive recursive file deletion".to_string());
    }
}

fn evaluate_raw_policy(input: &PolicyInput, warnings: &mut Vec<String>, denials: &mut Vec<String>) {
    let raw = normalize_token(&input.raw_display);
    for shell in ["bash", "sh", "powershell", "pwsh", "cmd"] {
        if contains_command_word(&raw, shell) {
            warnings.push(format!("nested shell invocation detected: {}", shell));
        }
    }

    if raw.contains("git checkout -b") || raw.contains("git branch") {
        let words: Vec<String> = raw.split_whitespace().map(|s| s.to_string()).collect();
        warn_branch_name(&words, warnings);
    }

    let chained = raw.contains(';') || raw.contains("&&") || raw.contains("||");
    let destructive = (raw.contains("rm -rf") && raw_contains_root_target(&raw))
        || (raw.contains("remove-item")
            && raw.contains("-recurse")
            && raw.contains("-force")
            && raw_contains_root_target(&raw))
        || ((raw.contains("rd /s /q") || raw.contains("rmdir /s /q"))
            && raw_contains_root_target(&raw))
        || (raw.contains("del /s /q") && raw_contains_root_target(&raw));

    if destructive {
        let reason = if chained {
            "denied destructive command in raw shell chain"
        } else {
            "denied destructive raw shell command"
        };
        denials.push(reason.to_string());
    }
}

fn warn_nested_shell(cmd: &str, warnings: &mut Vec<String>) {
    if ["bash", "sh", "powershell", "pwsh", "cmd", "cmd.exe"].contains(&cmd) {
        warnings.push(format!("nested shell invocation detected: {}", cmd));
    }
}

fn warn_branch_name(tokens: &[String], warnings: &mut Vec<String>) {
    let branch = if tokens.len() >= 4
        && tokens[0] == "git"
        && tokens[1] == "checkout"
        && tokens[2] == "-b"
    {
        Some(tokens[3].as_str())
    } else if tokens.len() >= 3 && tokens[0] == "git" && tokens[1] == "branch" {
        Some(tokens[2].as_str())
    } else {
        None
    };

    if let Some(branch) = branch {
        if !is_reasonable_branch_name(branch) {
            warnings.push(format!("branch name may violate conventions: {}", branch));
        }
    }
}

fn is_reasonable_branch_name(branch: &str) -> bool {
    let allowed_prefix = [
        "feat/",
        "fix/",
        "chore/",
        "docs/",
        "test/",
        "refactor/",
        "hotfix/",
    ]
    .iter()
    .any(|prefix| branch.starts_with(prefix));
    allowed_prefix
        && !branch.contains(' ')
        && !branch.contains('\\')
        && !branch.contains("..")
        && !branch.starts_with('/')
        && !branch.ends_with('/')
}

fn contains_command_word(raw: &str, needle: &str) -> bool {
    raw.split(|c: char| c.is_whitespace() || [';', '&', '|', '(', ')'].contains(&c))
        .any(|word| word == needle || word == format!("{}.exe", needle))
}

fn has_token(tokens: &[String], needle: &str) -> bool {
    tokens.iter().any(|token| token == needle)
}

fn has_any_token(tokens: &[String], needles: &[&str]) -> bool {
    needles.iter().any(|needle| has_token(tokens, needle))
}

fn has_rm_recursive_force(tokens: &[String]) -> bool {
    has_any_token(tokens, &["-rf", "-fr"]) || (has_token(tokens, "-r") && has_token(tokens, "-f"))
}

fn is_root_target(token: &str) -> bool {
    let token = token.trim_matches(|c| matches!(c, '"' | '\'' | ';' | '&' | '|' | '(' | ')'));
    matches!(token, "/" | "\\" | "c:\\" | "c:/" | "%systemdrive%\\")
}

fn raw_contains_root_target(raw: &str) -> bool {
    raw.split(|c: char| c.is_whitespace() || [';', '&', '|', '(', ')'].contains(&c))
        .any(is_root_target)
        || raw.contains(" / ")
        || raw.contains(" '/'")
        || raw.contains(" \"/\"")
        || raw.contains(" c:\\")
        || raw.contains(" c:/")
}

fn normalize_token(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_ascii_lowercase()
}

fn print_decision(decision: &PolicyDecision, input: &PolicyInput, verbose: bool) {
    match decision {
        PolicyDecision::Allow => println!("harness: allow"),
        PolicyDecision::Warn(reasons) => {
            println!("harness: warn");
            for reason in reasons {
                eprintln!("warning: {}", reason);
            }
        }
        PolicyDecision::Deny(reasons) => {
            eprintln!("harness: deny");
            for reason in reasons {
                eprintln!("denied: {}", reason);
            }
        }
    }

    if verbose {
        println!(
            "harness: {} command, policy matching {}",
            if input.raw_shell { "raw-shell" } else { "argv" },
            if input.raw_shell {
                "best-effort"
            } else {
                "argv-preserving"
            }
        );
    }
}

fn write_audit_log(
    input: &PolicyInput,
    decision: &PolicyDecision,
    execution_result: &str,
    exit_code: Option<i32>,
) -> Result<(), String> {
    let config_dir = crate::config::config_dir()
        .ok_or_else(|| "could not determine config directory".to_string())?;
    let log_dir = config_dir.join("logs");
    std::fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
    let log_path = log_dir.join("harness.log");

    let redacted = redact_secrets(&input.raw_display);
    let (command, command_truncated) = cap_logged_command(&redacted);
    let (decision_name, reasons) = decision_parts(decision);
    let entry = HarnessLogEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        identity: derive_identity(),
        identity_note: "unverified audit hint from process environment",
        raw_shell: input.raw_shell,
        command,
        command_truncated,
        argv_count: input.argv.len(),
        command_hash: command_hash(&redacted),
        decision: decision_name.to_string(),
        reasons,
        mode: if input.raw_shell {
            "raw-command"
        } else {
            "argv"
        }
        .to_string(),
        execution_result: execution_result.to_string(),
        exit_code,
    };

    let line = serde_json::to_string(&entry).map_err(|e| e.to_string())?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{}", line).map_err(|e| e.to_string())
}

fn decision_parts(decision: &PolicyDecision) -> (&'static str, Vec<String>) {
    match decision {
        PolicyDecision::Allow => ("allow", Vec::new()),
        PolicyDecision::Warn(reasons) => ("warn", reasons.clone()),
        PolicyDecision::Deny(reasons) => ("deny", reasons.clone()),
    }
}

fn derive_identity() -> String {
    let root = std::env::var("AGENTSCOMMANDER_ROOT").unwrap_or_default();
    if root.is_empty() {
        return "unidentified_agent".to_string();
    }

    std::path::Path::new(&root)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.trim_start_matches("__agent_").to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unidentified_agent".to_string())
}

fn cap_logged_command(command: &str) -> (String, bool) {
    if command.chars().count() <= MAX_LOGGED_COMMAND_LEN {
        return (command.to_string(), false);
    }
    let capped: String = command.chars().take(MAX_LOGGED_COMMAND_LEN).collect();
    (format!("{}...[truncated]", capped), true)
}

pub(crate) fn redact_secrets(command: &str) -> String {
    let mut out = Vec::new();
    let mut redact_next = false;
    for token in command.split_whitespace() {
        let lower = token.to_ascii_lowercase();
        if redact_next {
            out.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }
        if lower == "bearer" {
            out.push("Bearer".to_string());
            redact_next = true;
        } else if lower.starts_with("bearer ") {
            out.push("Bearer [REDACTED]".to_string());
        } else if is_secret_assignment(&lower) {
            out.push(redact_assignment(token));
            if lower.contains("bearer") {
                redact_next = true;
            }
        } else if is_secret_flag(&lower) {
            out.push(token.to_string());
            redact_next = true;
        } else {
            out.push(token.to_string());
        }
    }
    out.join(" ")
}

fn is_secret_assignment(lower: &str) -> bool {
    lower.contains("token=")
        || lower.contains("_authtoken=")
        || lower.contains("authorization=")
        || lower.contains("password=")
        || lower.contains("secret=")
        || lower.contains("api_key=")
        || lower.contains("apikey=")
}

fn is_secret_flag(lower: &str) -> bool {
    matches!(
        lower,
        "--token" | "--auth-token" | "--password" | "--secret" | "--api-key" | "-p"
    )
}

fn redact_assignment(token: &str) -> String {
    if let Some((key, _)) = token.split_once('=') {
        format!("{}=[REDACTED]", key)
    } else {
        "[REDACTED]".to_string()
    }
}

fn display_argv(argv: &[String]) -> String {
    argv.join("\u{1f}")
}

fn command_hash(command: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    command.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_destructive_unix_root_removal() {
        let input = PolicyInput {
            raw_display: display_argv(&["rm".into(), "-rf".into(), "/".into()]),
            argv: vec!["rm".into(), "-rf".into(), "/".into()],
            raw_shell: false,
        };
        assert!(matches!(evaluate_policy(&input), PolicyDecision::Deny(_)));
    }

    #[test]
    fn denies_destructive_windows_forms() {
        let inputs = [
            vec!["Remove-Item", "-Recurse", "-Force", "C:\\"],
            vec!["rd", "/s", "/q", "C:\\"],
            vec!["del", "/s", "/q", "C:\\"],
        ];
        for argv in inputs {
            let argv: Vec<String> = argv.into_iter().map(str::to_string).collect();
            let input = PolicyInput {
                raw_display: display_argv(&argv),
                argv,
                raw_shell: false,
            };
            assert!(matches!(evaluate_policy(&input), PolicyDecision::Deny(_)));
        }
    }

    #[test]
    fn raw_chain_destructive_command_is_denied() {
        let input = PolicyInput {
            raw_display: "echo ok && rm -rf /".to_string(),
            argv: Vec::new(),
            raw_shell: true,
        };
        assert!(matches!(evaluate_policy(&input), PolicyDecision::Deny(_)));
    }

    #[test]
    fn branch_guardrail_warns_without_denying() {
        let input = PolicyInput {
            raw_display: "git checkout -b bad branch".to_string(),
            argv: vec![
                "git".into(),
                "checkout".into(),
                "-b".into(),
                "bad branch".into(),
            ],
            raw_shell: false,
        };
        assert!(matches!(evaluate_policy(&input), PolicyDecision::Warn(_)));
    }

    #[test]
    fn nested_shell_warns() {
        let input = PolicyInput {
            raw_display: "pwsh -Command echo hi".to_string(),
            argv: vec!["pwsh".into(), "-Command".into(), "echo hi".into()],
            raw_shell: false,
        };
        assert!(matches!(evaluate_policy(&input), PolicyDecision::Warn(_)));
    }

    #[test]
    fn argv_display_preserves_argument_boundaries() {
        let argv = vec!["echo".to_string(), "a b".to_string(), "c\"d".to_string()];
        assert_eq!(display_argv(&argv), "echo\u{1f}a b\u{1f}c\"d");
    }

    #[test]
    fn redacts_secret_patterns() {
        let redacted = redact_secrets(
            "curl -H Authorization=Bearer abc --token secret _authToken=def password=hunter2",
        );
        assert!(redacted.contains("Authorization=[REDACTED]"));
        assert!(redacted.contains("--token [REDACTED]"));
        assert!(redacted.contains("_authToken=[REDACTED]"));
        assert!(redacted.contains("password=[REDACTED]"));
        assert!(!redacted.contains("abc"));
        assert!(!redacted.contains("hunter2"));
        assert!(!redacted.contains("secret _authToken"));
    }
}
