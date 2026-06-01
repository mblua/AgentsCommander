#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedAgentCommand {
    pub shell: String,
    pub shell_args: Vec<String>,
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

    for ch in input.chars() {
        match (quote, ch) {
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

#[cfg(test)]
mod tests {
    use super::normalize_legacy_agent_command;

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
    fn rejects_unclosed_quote() {
        let err = normalize_legacy_agent_command("codex \"unterminated").unwrap_err();
        assert!(err.contains("unclosed double quote"));
    }

    #[test]
    fn rejects_empty_quoted_executable() {
        let err = normalize_legacy_agent_command("\"\" --flag").unwrap_err();
        assert!(err.contains("agent executable is empty"));
    }
}
