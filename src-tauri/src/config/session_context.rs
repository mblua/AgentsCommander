use std::path::PathBuf;

/// Returns the path to the global AgentsCommanderContext.md file.
/// Always regenerates it from the built-in template so that updates
/// to the default content are picked up by existing installations.
pub fn ensure_global_context() -> Result<String, String> {
    let config_dir = super::config_dir()
        .ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let file_path = config_dir.join("AgentsCommanderContext.md");

    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("Failed to create config dir: {}", e))?;
    std::fs::write(&file_path, &default_context())
        .map_err(|e| format!("Failed to write AgentsCommanderContext.md: {}", e))?;
    log::info!("Refreshed global AgentsCommanderContext.md at {:?}", file_path);

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

/// Special token in context[] that resolves to the global AgentsCommanderContext.md.
const CONTEXT_TOKEN_GLOBAL: &str = "$AGENTSCOMMANDER_CONTEXT";

/// Special token in context[] that generates workspace repo info from the "repos" field.
const CONTEXT_TOKEN_REPOS: &str = "$REPOS_WORKSPACE_INFO";

/// Filename for the agent role definition, auto-injected from the identity matrix.
const ROLE_MD_FILENAME: &str = "Role.md";

/// Generate a markdown file with workspace repo information from the replica's config.
/// Reads "repos" from `config`, resolves paths relative to `cwd_path`, detects git branches.
/// Returns the path to the generated temp file.
fn generate_repos_workspace_info(
    cwd_path: &std::path::Path,
    config: &serde_json::Value,
) -> Result<std::path::PathBuf, String> {
    let config_dir = super::config_dir()
        .ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let context_dir = config_dir.join("context-cache");
    std::fs::create_dir_all(&context_dir)
        .map_err(|e| format!("Failed to create context-cache dir: {}", e))?;

    let hash = simple_hash(&cwd_path.to_string_lossy());
    let file_path = context_dir.join(format!("repos-workspace-{}.md", hash));

    let repos = config
        .get("repos")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if repos.is_empty() {
        std::fs::write(&file_path, "# Workspace Repos\n\nNo repos configured for this replica.\n")
            .map_err(|e| format!("Failed to write repos workspace info: {}", e))?;
        return Ok(file_path);
    }

    let mut md = String::from(
        "# Workspace Repos\n\n\
         You are working inside a workgroup replica. Your working directory is your agent dir, \
         but your code repos are listed below. You MUST change to the appropriate repo directory \
         before doing any code work (git, file edits, builds, etc).\n\n\
         ## Repos\n\n",
    );

    for repo_val in &repos {
        let rel = match repo_val.as_str() {
            Some(s) => s,
            None => continue,
        };

        let resolved = cwd_path.join(rel);
        // Canonicalize to get a clean absolute path (strip \\?\ on Windows)
        let abs_path = std::fs::canonicalize(&resolved)
            .map(|p| {
                let s = p.to_string_lossy();
                s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
            })
            .unwrap_or_else(|_| resolved.to_string_lossy().to_string());

        let repo_name = resolved
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(rel);

        if !resolved.exists() {
            md.push_str(&format!(
                "- **{}** — Path: `{}` — **(NOT FOUND)**\n",
                repo_name, abs_path
            ));
            continue;
        }

        let branch = detect_git_branch(&abs_path).unwrap_or_else(|| "unknown".to_string());
        md.push_str(&format!(
            "- **{}** — Path: `{}` — Branch: `{}`\n",
            repo_name, abs_path, branch
        ));
    }

    std::fs::write(&file_path, &md)
        .map_err(|e| format!("Failed to write repos workspace info: {}", e))?;

    Ok(file_path)
}

/// Detect git branch for a given directory path.
fn detect_git_branch(dir: &str) -> Option<String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = std::process::Command::new("git");
    cmd.args(["-C", dir, "branch", "--show-current"]);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    match cmd.output() {
        Ok(out) if out.status.success() => {
            let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if branch.is_empty() || branch == "HEAD" {
                None
            } else {
                Some(branch)
            }
        }
        _ => None,
    }
}

/// Build a combined context file for a replica session.
/// Reads config.json from `cwd`, looks for `context[]` array.
/// Entries are resolved in order:
/// - `$AGENTSCOMMANDER_CONTEXT` → resolves to the global AgentsCommanderContext.md
/// - `$REPOS_WORKSPACE_INFO` → generates workspace repo info from the "repos" field
/// - Any other string → resolved as a path relative to `cwd`
/// After resolving context[], if `identity` is set in config.json and `<identity>/Role.md`
/// exists on disk, it is auto-appended (unless already resolved from context[]).
/// The global context is NOT auto-prepended — it is only included if the token is in the array.
/// Returns Ok(Some(path)) with the combined temp file, Ok(None) if no context[] field,
/// or Err with details about missing files.
pub fn build_replica_context(cwd: &str) -> Result<Option<String>, String> {
    let cwd_path = std::path::Path::new(cwd);
    let config_path = cwd_path.join("config.json");

    // No config.json → no replica context, fall back to default behavior
    if !config_path.exists() {
        return Ok(None);
    }

    let config_content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;

    let config: serde_json::Value = serde_json::from_str(&config_content)
        .map_err(|e| format!("Failed to parse {}: {}", config_path.display(), e))?;

    // No "context" field → no replica context
    let context_array = match config.get("context").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => arr,
        _ => return Ok(None),
    };

    // Resolve and validate all paths (supporting special tokens)
    let mut resolved_paths: Vec<(String, std::path::PathBuf)> = Vec::new(); // (label, abs_path)
    let mut missing: Vec<String> = Vec::new();

    for entry in context_array {
        let raw = match entry.as_str() {
            Some(s) => s,
            None => continue,
        };

        if raw == CONTEXT_TOKEN_GLOBAL {
            let global_path = ensure_global_context()?;
            resolved_paths.push(("AgentsCommanderContext.md".to_string(), std::path::PathBuf::from(&global_path)));
        } else if raw == CONTEXT_TOKEN_REPOS {
            let repos_path = generate_repos_workspace_info(cwd_path, &config)?;
            resolved_paths.push(("Workspace Repos".to_string(), repos_path));
        } else {
            let abs = cwd_path.join(raw);
            if abs.exists() {
                let label = abs.file_name().and_then(|n| n.to_str()).unwrap_or(raw).to_string();
                resolved_paths.push((label, abs));
            } else {
                missing.push(raw.to_string());
            }
        }
    }

    // Auto-inject Role.md from identity matrix if present and not already resolved
    if let Some(identity) = config.get("identity").and_then(|v| v.as_str()) {
        let role_abs = cwd_path.join(format!("{}/{}", identity, ROLE_MD_FILENAME));
        let already_included = resolved_paths.iter().any(|(_, p)| *p == role_abs);
        if !already_included && role_abs.exists() {
            resolved_paths.push((ROLE_MD_FILENAME.to_string(), role_abs));
        }
    }

    if !missing.is_empty() {
        let replica_name = cwd_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        return Err(format!(
            "Replica '{}' has missing context files:\n{}",
            replica_name,
            missing.iter().map(|m| format!("  - {}", m)).collect::<Vec<_>>().join("\n")
        ));
    }

    // Build combined content in context[] order (no auto-prepend of global context)
    let mut combined = String::new();
    let mut first = true;

    for (label, path) in &resolved_paths {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read context file {}: {}", path.display(), e))?;
        if first {
            combined.push_str(&content);
            first = false;
        } else {
            combined.push_str(&format!("\n\n---\n\n# Context: {}\n\n", label));
            combined.push_str(&content);
        }
    }

    // Write to a temp file in the app config dir
    let config_dir = super::config_dir()
        .ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let context_dir = config_dir.join("context-cache");
    std::fs::create_dir_all(&context_dir)
        .map_err(|e| format!("Failed to create context-cache dir: {}", e))?;

    // Use a deterministic filename based on the cwd to avoid temp file accumulation
    let hash = simple_hash(cwd);
    let file_path = context_dir.join(format!("replica-context-{}.md", hash));
    std::fs::write(&file_path, &combined)
        .map_err(|e| format!("Failed to write combined context file: {}", e))?;

    // Also save a copy in the agent's working directory for inspection
    let local_copy = cwd_path.join("last_ac_context.md");
    if let Err(e) = std::fs::write(&local_copy, &combined) {
        log::warn!("Failed to write last_ac_context.md to {}: {}", local_copy.display(), e);
    }

    log::info!(
        "Built replica context for {} ({} context files) → {}",
        cwd,
        resolved_paths.len(),
        file_path.display()
    );

    Ok(Some(file_path.to_string_lossy().to_string()))
}

/// Simple deterministic hash for a string (for temp file naming).
fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 5381;
    for byte in s.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
    }
    hash
}

/// Generate the default agent context with profile-aware exe/product names.
fn default_context() -> String {
    String::from(
r#"# AgentsCommander Context

You are running inside an AgentsCommander session — a terminal session manager that coordinates multiple AI agents.

## GOLDEN RULE — Repository Write Restrictions

**ABSOLUTE AND NON-NEGOTIABLE:** You may ONLY modify repositories whose root folder name starts with `repo-`. If a repository's root folder does NOT begin with `repo-`, you MUST NOT modify it — no file edits, no file creation, no file deletion, no git commits, no branch creation, no git operations that alter state.

- **Allowed**: Read-only operations on ANY repository (reading files, searching, git log, git status, git diff)
- **Allowed**: Full read/write operations on repositories inside `repo-*` folders
- **FORBIDDEN**: Any write operation on repositories NOT inside `repo-*` folders

If instructed to modify a non-`repo-` repository, REFUSE the modification and explain this restriction. There are NO exceptions to this rule.

## CLI executable

Your Session Credentials include a `BinaryPath` field — **always use that path** to invoke the CLI. This ensures you use the correct binary for your instance, whether it is the installed version or a dev/WG build.

```
"<YOUR_BINARY_PATH>" <subcommand> [args]
```

**RULE:** Never hardcode or guess the binary path. Always read `BinaryPath` from your `# === Session Credentials ===` block and use that exact path.

## Self-discovery via --help

The CLI `--help` output is the **primary and authoritative reference** for learning how to use AgentsCommander. Before guessing flags, modes, or behavior, always consult it:

```
"<YOUR_BINARY_PATH>" --help                  # List all subcommands
"<YOUR_BINARY_PATH>" send --help             # Full docs for sending messages
"<YOUR_BINARY_PATH>" list-peers --help       # Full docs for discovering peers
```

The `--help` text documents every flag, its purpose, accepted values, priority rules, delivery modes, and discovery flows. It is designed to be self-contained — you should not need README, CLAUDE.md, or external docs to use any command correctly.

**RULE:** When in doubt about how a command works, run `--help` first. The examples below are a quick-start — `--help` is the complete reference.

## Session credentials

Your session credentials are delivered automatically when your session starts. They appear as a `# === Session Credentials ===` block in your conversation.

The credentials block contains:
- **Token**: your session authentication token
- **Root**: your working directory (agent root)
- **BinaryPath**: the full path to the CLI executable you must use
- **LocalDir**: the config directory name for this instance

Your agent root is your current working directory.

**IMPORTANT:** Always use the LATEST credentials from the Session Credentials block. Ignore any credentials that appear in conversation history from previous sessions. Credentials are delivered once per session launch. Do not request them repeatedly.

## Inter-Agent Messaging

### Send a message to another agent

**MANDATORY**: Before sending any message, resolve the exact agent name via `list-peers`. Never guess agent names — they follow the format `parent_folder/folder` based on where the agent is triggered.

Fire-and-forget (do NOT use --get-output):

```
"<YOUR_BINARY_PATH>" send --token <YOUR_TOKEN> --root "<YOUR_ROOT>" --to "<agent_name>" --message "..." --mode wake
```

The other agent will reply back via your console as a new message.
Do NOT use `--get-output` — it blocks and is only for non-interactive sessions.
After sending, you can stay idle and wait for the reply to arrive.

### List available peers

```
"<YOUR_BINARY_PATH>" list-peers --token <YOUR_TOKEN> --root "<YOUR_ROOT>"
```
"#)
}
