use std::path::PathBuf;

/// Returns the path to the global AgentsCommanderContext.md file.
/// Creates it with default content if it doesn't exist yet.
/// This file is static — written once, never modified at runtime.
pub fn ensure_global_context() -> Result<String, String> {
    let config_dir = super::config_dir()
        .ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let file_path = config_dir.join("AgentsCommanderContext.md");

    if !file_path.exists() {
        std::fs::create_dir_all(&config_dir)
            .map_err(|e| format!("Failed to create config dir: {}", e))?;
        std::fs::write(&file_path, DEFAULT_CONTEXT)
            .map_err(|e| format!("Failed to write AgentsCommanderContext.md: {}", e))?;
        log::info!("Created global AgentsCommanderContext.md at {:?}", file_path);
    }

    Ok(file_path.to_string_lossy().to_string())
}

/// Returns the expected path without creating the file.
pub fn global_context_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("AgentsCommanderContext.md"))
}

const AC_START_MARKER: &str = "# === AgentsCommander Context START ===";
const AC_END_MARKER: &str = "# === AgentsCommander Context END ===";

/// Ensures the Codex user-level config at ~/.codex/config.toml contains
/// the AgentsCommander context as `developer_instructions`.
/// Uses start/end markers to preserve any existing user content in the field.
pub fn ensure_codex_context() -> Result<(), String> {
    // 1. Ensure AgentsCommanderContext.md exists and read its content
    let context_path = ensure_global_context()?;
    let context_content = std::fs::read_to_string(&context_path)
        .map_err(|e| format!("Failed to read AgentsCommanderContext.md: {}", e))?;

    // 2. Resolve ~/.codex/config.toml
    let codex_dir = dirs::home_dir()
        .ok_or_else(|| "Could not resolve home directory".to_string())?
        .join(".codex");
    let config_path = codex_dir.join("config.toml");

    // 3. Read existing config or start with empty table
    let mut table: toml::value::Table = if config_path.exists() {
        let raw = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read ~/.codex/config.toml: {}", e))?;
        raw.parse::<toml::Value>()
            .map_err(|e| format!("Failed to parse ~/.codex/config.toml: {}", e))?
            .as_table()
            .cloned()
            .unwrap_or_default()
    } else {
        toml::value::Table::new()
    };

    // 4. Build the marked AC block
    let ac_block = format!(
        "{}\n{}\n{}",
        AC_START_MARKER,
        context_content.trim(),
        AC_END_MARKER,
    );

    // 5. Merge with existing developer_instructions (preserve user content outside markers)
    let current_di = table
        .get("developer_instructions")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let new_di = replace_ac_block(current_di, &ac_block);

    // 6. Skip write if nothing changed
    if new_di == current_di {
        log::debug!("Codex developer_instructions already up to date, skipping write");
        return Ok(());
    }

    // 7. Write back
    table.insert(
        "developer_instructions".to_string(),
        toml::Value::String(new_di),
    );
    std::fs::create_dir_all(&codex_dir)
        .map_err(|e| format!("Failed to create ~/.codex/ directory: {}", e))?;
    let serialized = toml::to_string(&toml::Value::Table(table))
        .map_err(|e| format!("Failed to serialize ~/.codex/config.toml: {}", e))?;
    std::fs::write(&config_path, &serialized)
        .map_err(|e| format!("Failed to write ~/.codex/config.toml: {}", e))?;

    log::info!("Injected AgentsCommander context into ~/.codex/config.toml developer_instructions");
    Ok(())
}

/// Replace (or insert) the AgentsCommander marked block within an existing string,
/// preserving any content outside the markers.
fn replace_ac_block(existing: &str, new_block: &str) -> String {
    if let Some(start) = existing.find(AC_START_MARKER) {
        if let Some(end_rel) = existing[start..].find(AC_END_MARKER) {
            let end = start + end_rel + AC_END_MARKER.len();
            let before = existing[..start].trim_end_matches('\n');
            let after = existing[end..].trim_start_matches('\n');

            let mut result = String::new();
            if !before.is_empty() {
                result.push_str(before);
                result.push('\n');
            }
            result.push_str(new_block);
            if !after.is_empty() {
                result.push('\n');
                result.push_str(after);
            }
            return result;
        }
    }

    // No existing block — prepend if there's user content, or just the block
    if existing.trim().is_empty() {
        new_block.to_string()
    } else {
        format!("{}\n\n{}", new_block, existing)
    }
}

const DEFAULT_CONTEXT: &str = r#"# AgentsCommander Context

You are running inside an AgentsCommander session — a terminal session manager that coordinates multiple AI agents.

## CLI executable

`agentscommander.exe` is **not** in PATH. Use the full path via the `LOCALAPPDATA` environment variable (the directory name contains a space, so always quote):

```
"$LOCALAPPDATA/Agents Commander/agentscommander.exe"
```

## Self-discovery via --help

The CLI `--help` output is the **primary and authoritative reference** for learning how to use AgentsCommander. Before guessing flags, modes, or behavior, always consult it:

```
agentscommander.exe --help                  # List all subcommands
agentscommander.exe send --help             # Full docs for sending messages
agentscommander.exe list-peers --help       # Full docs for discovering peers
```

The `--help` text documents every flag, its purpose, accepted values, priority rules, delivery modes, and discovery flows. It is designed to be self-contained — you should not need README, CLAUDE.md, or external docs to use any command correctly.

**RULE:** When in doubt about how a command works, run `--help` first. The examples below are a quick-start — `--help` is the complete reference.

## Session credentials

Your session token and agent root are provided on demand. To request them, output the marker:

```
%%ACRC%%
```

The system will inject a `# === Session Credentials ===` block into your console containing your current token and root. This also happens automatically whenever a `send` command fails due to a stale or missing token.

Your agent root is your current working directory.

## Inter-Agent Messaging

### Send a message to another agent

**MANDATORY**: Before sending any message, resolve the exact agent name via `list-peers`. Never guess agent names — they follow the format `parent_folder/folder` based on where the agent is triggered.

Fire-and-forget (do NOT use --get-output):

```
agentscommander.exe send --token <YOUR_TOKEN> --root "<YOUR_ROOT>" --to "<agent_name>" --message "..." --mode wake
```

The other agent will reply back via your console as a new message.
Do NOT use `--get-output` — it blocks and is only for non-interactive sessions.
After sending, you can stay idle and wait for the reply to arrive.

### List available peers

```
agentscommander.exe list-peers --token <YOUR_TOKEN> --root "<YOUR_ROOT>"
```
"#;
