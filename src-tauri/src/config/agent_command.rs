use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::coding_agent_profiles::{
    resolve_profile, ProfileResolution, ProfileResolutionRequest,
};
use crate::config::placeholders::{
    ac_root_error, expand_placeholders, expand_placeholders_in_args,
    placeholder_context_for_launch_root, reject_unexpanded_markers, value_contains_ac_root,
    PlaceholderContext,
};
use crate::config::settings::{
    is_codex_home_key, normalize_env_key_for_platform, validate_expanded_codex_home_value,
    validate_user_env_key, AgentConfig, AppSettings,
};
use crate::session::profile::CodingAgentKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedAgentCommand {
    pub shell: String,
    pub shell_args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AgentSpawnCommand {
    pub shell: String,
    pub shell_args: Vec<String>,
    pub agent_env: BTreeMap<String, String>,
    pub profile_env: BTreeMap<String, String>,
    pub generated_env: BTreeMap<String, String>,
    pub child_env: Vec<(String, String)>,
    pub env_remove_keys: Vec<String>,
    pub effective_codex_home: Option<PathBuf>,
    pub profile_resolution: ProfileResolution,
    pub trusted_agent_id: String,
    pub trusted_agent_label: String,
}

pub fn normalize_legacy_agent_command(command: &str) -> Result<NormalizedAgentCommand, String> {
    let input = command.trim_matches(|c: char| c.is_ascii_whitespace());
    if input.is_empty() {
        return Err("agent command is empty".to_string());
    }

    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut token_started = false;

    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some('"'), '\\') => {
                if let Some(next) = chars.peek().copied() {
                    if next == '"' || next == '\\' {
                        current.push(chars.next().expect("peeked char exists"));
                    } else {
                        current.push('\\');
                    }
                } else {
                    current.push('\\');
                }
                token_started = true;
            }
            (Some(q), c) if c == q => {
                quote = None;
                token_started = true;
            }
            (Some(_), c) => {
                current.push(c);
                token_started = true;
            }
            (None, '\'' | '"') => {
                quote = Some(ch);
                token_started = true;
            }
            (None, c) if c.is_ascii_whitespace() => {
                if token_started {
                    tokens.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            (None, c) => {
                current.push(c);
                token_started = true;
            }
        }
    }

    if let Some(q) = quote {
        return Err(format!(
            "unclosed {} quote",
            if q == '"' { "double" } else { "single" }
        ));
    }

    if token_started {
        tokens.push(current);
    }

    let Some((shell, args)) = tokens.split_first() else {
        return Err("agent command is empty".to_string());
    };
    if shell.is_empty() {
        return Err("agent executable is empty".to_string());
    }

    Ok(NormalizedAgentCommand {
        shell: shell.clone(),
        shell_args: args.to_vec(),
    })
}

pub fn stringify_agent_command_tokens(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|token| stringify_agent_command_token(token))
        .collect::<Vec<_>>()
        .join(" ")
}

fn stringify_agent_command_token(token: &str) -> String {
    if token.is_empty() {
        return "\"\"".to_string();
    }
    if !token
        .chars()
        .any(|ch| ch.is_ascii_whitespace() || ch == '"' || ch == '\\')
    {
        return token.to_string();
    }
    let mut out = String::with_capacity(token.len() + 2);
    out.push('"');
    for ch in token.chars() {
        if ch == '"' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

fn executable_basename(s: &str) -> String {
    std::path::Path::new(s)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(s)
        .to_lowercase()
}

fn collect_agent_env(
    agent: &AgentConfig,
    placeholder_context: Option<&PlaceholderContext>,
) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let mut seen = BTreeMap::new();
    for row in agent.envs.iter().filter(|row| row.enabled) {
        let context = format!("Agent '{}' env settings", agent.label);
        validate_user_env_key(&row.key, &context)?;
        let normalized = normalize_env_key_for_platform(&row.key);
        if let Some(previous) = seen.insert(normalized, row.key.clone()) {
            return Err(format!(
                "{}: duplicate env keys '{}' and '{}'",
                context, previous, row.key
            ));
        }
        let strict_path = is_codex_home_key(&row.key);
        let value = expand_runtime_value(&row.value, placeholder_context, &context, strict_path)?;
        if strict_path {
            validate_expanded_codex_home_value(&value, &context)?;
        }
        out.insert(row.key.clone(), value);
    }
    Ok(out)
}

fn collect_profile_env(
    agent_id: &str,
    profile: &str,
    env: &BTreeMap<String, String>,
    placeholder_context: Option<&PlaceholderContext>,
) -> Result<BTreeMap<String, String>, String> {
    let context = format!("Profile '{}:{}' env settings", agent_id, profile);
    let mut out = BTreeMap::new();
    let mut seen = BTreeMap::new();
    for (key, value) in env {
        validate_user_env_key(key, &context)?;
        let normalized = normalize_env_key_for_platform(key);
        if let Some(previous) = seen.insert(normalized, key.clone()) {
            return Err(format!(
                "{}: duplicate env keys '{}' and '{}'",
                context, previous, key
            ));
        }
        let strict_path = is_codex_home_key(key);
        let value = expand_runtime_value(value, placeholder_context, &context, strict_path)?;
        if strict_path {
            validate_expanded_codex_home_value(&value, &context)?;
        }
        out.insert(key.clone(), value);
    }
    Ok(out)
}

fn expand_runtime_value(
    value: &str,
    placeholder_context: Option<&PlaceholderContext>,
    context: &str,
    strict_path_value: bool,
) -> Result<String, String> {
    match placeholder_context {
        Some(context_obj) => {
            let expanded = expand_placeholders(value, context_obj)?;
            reject_unexpanded_markers(&expanded, context, strict_path_value)?;
            Ok(expanded)
        }
        None => {
            if value_contains_ac_root(value) {
                return Err(ac_root_error().to_string());
            }
            reject_unexpanded_markers(value, context, strict_path_value)?;
            Ok(value.to_string())
        }
    }
}

fn find_env_value<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Option<&'a String> {
    let wanted = normalize_env_key_for_platform(key);
    env.iter()
        .find(|(candidate, _)| normalize_env_key_for_platform(candidate) == wanted)
        .map(|(_, value)| value)
}

fn sanitize_codex_home_id(agent_id: &str) -> String {
    let sanitized: String = agent_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "codex".to_string()
    } else {
        trimmed.to_string()
    }
}

struct ComputedCodexHome {
    generated_env: BTreeMap<String, String>,
    effective_codex_home: Option<PathBuf>,
    env_remove_keys: Vec<String>,
}

fn compute_codex_home(
    agent: &AgentConfig,
    shell: &str,
    shell_args: &[String],
    agent_env: &BTreeMap<String, String>,
    profile_env: &BTreeMap<String, String>,
) -> Result<ComputedCodexHome, String> {
    let mut generated_env = BTreeMap::new();
    let mut env_remove_keys = Vec::new();
    let agent_kind = CodingAgentKind::detect(shell, shell_args);
    if agent_kind != Some(CodingAgentKind::Codex) && executable_basename(shell) != "codex" {
        return Ok(ComputedCodexHome {
            generated_env,
            effective_codex_home: None,
            env_remove_keys,
        });
    }

    if agent.isolated_home {
        let config_dir = crate::config::config_dir()
            .ok_or_else(|| "Could not determine config directory for CODEX_HOME".to_string())?;
        let home = config_dir
            .join("codex-home")
            .join(sanitize_codex_home_id(&agent.id));
        std::fs::create_dir_all(&home).map_err(|e| {
            format!(
                "Failed to create isolated CODEX_HOME '{}': {}",
                home.display(),
                e
            )
        })?;
        generated_env.insert("CODEX_HOME".to_string(), home.to_string_lossy().to_string());
        return Ok(ComputedCodexHome {
            generated_env,
            effective_codex_home: Some(home),
            env_remove_keys,
        });
    }

    if let Some(value) = find_env_value(profile_env, "CODEX_HOME") {
        let path = validate_expanded_codex_home_value(value, "Profile CODEX_HOME")?;
        return Ok(ComputedCodexHome {
            generated_env,
            effective_codex_home: Some(path),
            env_remove_keys,
        });
    }
    if let Some(value) = find_env_value(agent_env, "CODEX_HOME") {
        let path = validate_expanded_codex_home_value(value, "Agent CODEX_HOME")?;
        return Ok(ComputedCodexHome {
            generated_env,
            effective_codex_home: Some(path),
            env_remove_keys,
        });
    }
    if let Ok(value) = std::env::var("CODEX_HOME") {
        match validate_expanded_codex_home_value(&value, "Inherited CODEX_HOME") {
            Ok(path) => {
                return Ok(ComputedCodexHome {
                    generated_env,
                    effective_codex_home: Some(path),
                    env_remove_keys,
                })
            }
            Err(e) => {
                log::warn!(
                    "[codex-home] {}. Removing inherited CODEX_HOME for child.",
                    e
                );
                env_remove_keys.push("CODEX_HOME".to_string());
            }
        }
    }

    Ok(ComputedCodexHome {
        generated_env,
        effective_codex_home: None,
        env_remove_keys,
    })
}

fn merge_env_layers(layers: &[&BTreeMap<String, String>]) -> Vec<(String, String)> {
    let mut by_normalized: BTreeMap<String, (String, String)> = BTreeMap::new();
    for layer in layers {
        for (key, value) in *layer {
            by_normalized.insert(
                normalize_env_key_for_platform(key),
                (key.clone(), value.clone()),
            );
        }
    }
    by_normalized.into_values().collect()
}

#[cfg(windows)]
fn cmd_quote_token(token: &str) -> Result<String, String> {
    if token.contains('\0') || token.contains('\n') || token.contains('\r') {
        return Err("gitPullBefore command tokens must not contain NUL or newline".to_string());
    }
    if token.contains('%') || token.contains('!') {
        return Err(
            "gitPullBefore command tokens must not contain percent or delayed-expansion markers"
                .to_string(),
        );
    }
    if token.contains('"') {
        return Err(
            "gitPullBefore command tokens with double quotes are not supported".to_string(),
        );
    }
    let escaped = token.replace('^', "^^");
    if escaped.is_empty() {
        return Ok("\"\"".to_string());
    }
    if escaped
        .chars()
        .any(|ch| ch.is_ascii_whitespace() || matches!(ch, '&' | '|' | '<' | '>' | '(' | ')'))
    {
        Ok(format!("\"{}\"", escaped))
    } else {
        Ok(escaped)
    }
}

#[cfg(windows)]
fn wrap_git_pull_before(
    shell: String,
    shell_args: Vec<String>,
) -> Result<(String, Vec<String>), String> {
    let mut parts = Vec::with_capacity(shell_args.len() + 1);
    parts.push(cmd_quote_token(&shell)?);
    for arg in &shell_args {
        parts.push(cmd_quote_token(arg)?);
    }
    Ok((
        "cmd.exe".to_string(),
        vec!["/K".to_string(), format!("git pull && {}", parts.join(" "))],
    ))
}

#[cfg(not(windows))]
fn wrap_git_pull_before(
    shell: String,
    shell_args: Vec<String>,
) -> Result<(String, Vec<String>), String> {
    Ok((shell, shell_args))
}

pub fn build_agent_spawn_command(
    settings: &AppSettings,
    agent_id: &str,
    launch_path: Option<&Path>,
    requested_profile: Option<&str>,
) -> Result<AgentSpawnCommand, String> {
    let agent = settings
        .agents
        .iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| format!("Agent '{}' is not configured", agent_id))?;
    let profile_resolution = resolve_profile(
        settings,
        ProfileResolutionRequest {
            coding_agent_id: &agent.id,
            launch_path,
            agent_matrix_name: None,
            requested_profile,
        },
    );
    for warning in &profile_resolution.warnings {
        log::warn!("[profiles] {}", warning);
    }

    let selected_command = if profile_resolution.cell.command.trim().is_empty() {
        agent.command.as_str()
    } else {
        profile_resolution.cell.command.as_str()
    };
    let normalized = normalize_legacy_agent_command(selected_command).map_err(|e| {
        format!(
            "Invalid profile command for '{}:{}': {}. command={:?}",
            agent.id, profile_resolution.effective_profile, e, selected_command
        )
    })?;
    let mut command_tokens = Vec::with_capacity(normalized.shell_args.len() + 1);
    command_tokens.push(normalized.shell);
    command_tokens.extend(normalized.shell_args);
    let needs_placeholder_context = command_tokens
        .iter()
        .any(|value| value_contains_ac_root(value))
        || agent
            .envs
            .iter()
            .filter(|row| row.enabled)
            .any(|row| value_contains_ac_root(&row.value))
        || profile_resolution
            .cell
            .env
            .values()
            .any(|value| value_contains_ac_root(value));
    let placeholder_context = if needs_placeholder_context {
        Some(
            launch_path
                .ok_or_else(|| ac_root_error().to_string())
                .and_then(placeholder_context_for_launch_root)?,
        )
    } else {
        None
    };
    let expanded_command_tokens = if let Some(context) = placeholder_context.as_ref() {
        expand_placeholders_in_args(&command_tokens, context)?
    } else {
        for token in &command_tokens {
            reject_unexpanded_markers(token, "Agent command", false)?;
        }
        command_tokens
    };
    let Some((shell, shell_args)) = expanded_command_tokens.split_first() else {
        return Err("agent command is empty".to_string());
    };
    let mut shell = shell.clone();
    let mut shell_args = shell_args.to_vec();

    let agent_env = collect_agent_env(agent, placeholder_context.as_ref())?;
    let profile_env = collect_profile_env(
        &agent.id,
        &profile_resolution.effective_profile,
        &profile_resolution.cell.env,
        placeholder_context.as_ref(),
    )?;
    let computed_codex_home =
        compute_codex_home(agent, &shell, &shell_args, &agent_env, &profile_env)?;
    let child_env =
        merge_env_layers(&[&agent_env, &profile_env, &computed_codex_home.generated_env]);

    if agent.git_pull_before {
        let wrapped = wrap_git_pull_before(shell, shell_args)?;
        shell = wrapped.0;
        shell_args = wrapped.1;
    }

    Ok(AgentSpawnCommand {
        shell,
        shell_args,
        agent_env,
        profile_env,
        generated_env: computed_codex_home.generated_env,
        child_env,
        env_remove_keys: computed_codex_home.env_remove_keys,
        effective_codex_home: computed_codex_home.effective_codex_home,
        profile_resolution,
        trusted_agent_id: agent.id.clone(),
        trusted_agent_label: agent.label.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::{build_agent_spawn_command, normalize_legacy_agent_command};
    use crate::config::settings::{
        AgentConfig, AppSettings, CodingAgentEnv, CodingAgentEnvSource, ProfileCellConfig,
    };
    use std::collections::BTreeMap;

    #[test]
    fn normalizes_plain_command_with_args() {
        let got = normalize_legacy_agent_command("codex --yolo").unwrap();
        assert_eq!(got.shell, "codex");
        assert_eq!(got.shell_args, vec!["--yolo"]);
    }

    #[test]
    fn preserves_quoted_arg_with_spaces() {
        let got = normalize_legacy_agent_command("codex --model \"gpt 5\"").unwrap();
        assert_eq!(got.shell, "codex");
        assert_eq!(got.shell_args, vec!["--model", "gpt 5"]);
    }

    #[test]
    fn supports_quoted_windows_executable_path() {
        let got = normalize_legacy_agent_command("\"C:\\Program Files\\Codex\\codex.exe\" --yolo")
            .unwrap();
        assert_eq!(got.shell, "C:\\Program Files\\Codex\\codex.exe");
        assert_eq!(got.shell_args, vec!["--yolo"]);
    }

    #[test]
    fn preserves_empty_quoted_arg() {
        let got = normalize_legacy_agent_command("codex --config \"\" --flag").unwrap();
        assert_eq!(got.shell, "codex");
        assert_eq!(got.shell_args, vec!["--config", "", "--flag"]);
    }

    #[test]
    fn stringifies_migration_tokens_with_spaces_quotes_and_empty_args() {
        let tokens = vec![
            "C:\\Program Files\\Codex\\codex.exe".to_string(),
            "--config".to_string(),
            "".to_string(),
            "a\"b".to_string(),
        ];
        let text = super::stringify_agent_command_tokens(&tokens);
        let normalized = normalize_legacy_agent_command(&text).unwrap();
        let mut round_trip = vec![normalized.shell];
        round_trip.extend(normalized.shell_args);
        assert_eq!(round_trip, tokens);
    }

    #[test]
    fn rejects_unclosed_quote() {
        let err = normalize_legacy_agent_command("codex \"unterminated").unwrap_err();
        assert!(err.contains("unclosed double quote"));
    }

    #[test]
    fn rejects_empty_quoted_executable() {
        let err = normalize_legacy_agent_command("\"\" --flag").unwrap_err();
        assert!(err.contains("agent executable is empty"));
    }

    fn agent(id: &str, command: &str) -> AgentConfig {
        AgentConfig {
            id: id.to_string(),
            label: id.to_string(),
            command: command.to_string(),
            color: "#000000".to_string(),
            git_pull_before: false,
            exclude_global_claude_md: false,
            envs: Vec::new(),
            isolated_home: false,
        }
    }

    #[test]
    fn build_spawn_uses_profile_command_as_complete_invocation() {
        let mut settings = AppSettings {
            agents: vec![agent("codex", "codex --base")],
            ..AppSettings::default()
        };
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                "B".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "codex --profile fast".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );

        let spawn = build_agent_spawn_command(&settings, "codex", None, Some("B")).unwrap();

        assert_eq!(spawn.shell, "codex");
        assert_eq!(spawn.shell_args, vec!["--profile", "fast"]);
        assert_eq!(spawn.profile_resolution.effective_profile, "B");
    }

    #[test]
    fn env_merge_precedence_is_agent_profile_generated() {
        let mut codex = agent("codex", "codex");
        codex.envs = vec![
            CodingAgentEnv {
                key: "OPENAI_API_BASE".to_string(),
                value: "agent".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
            CodingAgentEnv {
                key: "CODEX_HOME".to_string(),
                value: std::env::temp_dir()
                    .join("agent-codex")
                    .to_string_lossy()
                    .to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
        ];
        codex.isolated_home = true;
        let mut settings = AppSettings {
            agents: vec![codex],
            ..AppSettings::default()
        };
        let profile_home = std::env::temp_dir().join("profile-codex");
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: String::new(),
                    env: BTreeMap::from([
                        ("OPENAI_API_BASE".to_string(), "profile".to_string()),
                        (
                            "CODEX_HOME".to_string(),
                            profile_home.to_string_lossy().to_string(),
                        ),
                    ]),
                    notes: String::new(),
                },
            );

        let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();
        let env: BTreeMap<_, _> = spawn.child_env.into_iter().collect();

        assert_eq!(
            env.get("OPENAI_API_BASE").map(String::as_str),
            Some("profile")
        );
        assert_eq!(
            env.get("CODEX_HOME").map(String::as_str),
            spawn
                .effective_codex_home
                .as_ref()
                .map(|path| path.to_string_lossy().to_string())
                .as_deref()
        );
        assert!(spawn.generated_env.contains_key("CODEX_HOME"));
    }

    #[test]
    fn reserved_env_keys_are_rejected_case_insensitively_on_windows() {
        let mut codex = agent("codex", "codex");
        codex.envs = vec![CodingAgentEnv {
            key: if cfg!(windows) {
                "agentscommander_token".to_string()
            } else {
                "AGENTSCOMMANDER_TOKEN".to_string()
            },
            value: "bad".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        let settings = AppSettings {
            agents: vec![codex],
            ..AppSettings::default()
        };

        let err = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap_err();
        assert!(err.contains("reserved"), "{err}");
    }

    #[test]
    fn profile_codex_home_is_effective_when_isolation_is_off() {
        let profile_home = std::env::temp_dir().join("profile-codex-home");
        let mut settings = AppSettings {
            agents: vec![agent("codex", "codex")],
            ..AppSettings::default()
        };
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: String::new(),
                    env: BTreeMap::from([(
                        "CODEX_HOME".to_string(),
                        profile_home.to_string_lossy().to_string(),
                    )]),
                    notes: String::new(),
                },
            );

        let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();

        assert_eq!(
            spawn.effective_codex_home.as_deref(),
            Some(profile_home.as_path())
        );
        assert!(spawn.generated_env.is_empty());
    }

    #[test]
    fn ac_root_command_expands_after_parse_for_replica_launch_root() {
        let temp = tempfile::tempdir().unwrap();
        let replica = temp
            .path()
            .join("root with spaces")
            .join(".ac")
            .join("wg-7-dev-team")
            .join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).unwrap();
        let mut settings = AppSettings {
            agents: vec![agent("codex", "codex")],
            ..AppSettings::default()
        };
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "%AC_ROOT%\\bin\\codex.exe --flag".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );

        let spawn = build_agent_spawn_command(&settings, "codex", Some(&replica), Some("A"))
            .expect("replica launch root should expand");

        assert_eq!(
            spawn.shell,
            replica.join("bin").join("codex.exe").to_string_lossy()
        );
        assert_eq!(spawn.shell_args, vec!["--flag"]);
    }

    #[test]
    fn ac_root_in_normal_repo_launch_returns_clear_error() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo-thing");
        std::fs::create_dir_all(&repo).unwrap();
        let settings = AppSettings {
            agents: vec![agent("codex", "%AC_ROOT%\\bin\\codex.exe")],
            ..AppSettings::default()
        };

        let err = build_agent_spawn_command(&settings, "codex", Some(&repo), Some("A"))
            .expect_err("normal repo launch must reject AC_ROOT");

        assert!(err.contains("%AC_ROOT% requires an AC replica or root-agent launch root"));
    }

    #[test]
    fn non_path_env_allows_literal_dollar_but_rejects_unknown_percent_marker() {
        let mut codex = agent("codex", "codex");
        codex.envs = vec![
            CodingAgentEnv {
                key: "SECRET".to_string(),
                value: "s3cr$tP4ss".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
            CodingAgentEnv {
                key: "BAD_MARKER".to_string(),
                value: "%UNKNOWN%".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
        ];
        let settings = AppSettings {
            agents: vec![codex],
            ..AppSettings::default()
        };

        let err = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap_err();

        assert!(err.contains("unknown placeholder"), "{err}");
    }

    #[cfg(windows)]
    #[test]
    fn duplicate_env_keys_are_rejected_case_insensitively_on_windows() {
        let mut codex = agent("codex", "codex");
        codex.envs = vec![
            CodingAgentEnv {
                key: "OPENAI_API_BASE".to_string(),
                value: "one".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
            CodingAgentEnv {
                key: "openai_api_base".to_string(),
                value: "two".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
        ];
        let settings = AppSettings {
            agents: vec![codex],
            ..AppSettings::default()
        };

        let err = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap_err();

        assert!(err.contains("duplicate env keys"), "{err}");
    }

    #[cfg(windows)]
    #[test]
    fn git_pull_before_wraps_tokens_without_raw_concatenation() {
        let mut codex = agent("codex", "codex --config effort=low");
        codex.git_pull_before = true;
        let settings = AppSettings {
            agents: vec![codex],
            ..AppSettings::default()
        };

        let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();

        assert_eq!(spawn.shell, "cmd.exe");
        assert_eq!(spawn.shell_args[0], "/K");
        assert_eq!(spawn.shell_args[1], "git pull && codex --config effort=low");
    }

    #[cfg(windows)]
    #[test]
    fn git_pull_before_rejects_expansion_tokens() {
        let mut codex = agent("codex", "codex --config %USERPROFILE%");
        codex.git_pull_before = true;
        let settings = AppSettings {
            agents: vec![codex],
            ..AppSettings::default()
        };

        let err = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap_err();
        assert!(err.contains("unknown placeholder"), "{err}");
    }
}
