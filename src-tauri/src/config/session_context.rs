/// Writes a per-agent copy of AgentsCommanderContext.md with the agent's own
/// root path interpolated into the GOLDEN RULE. Uses a deterministic filename
/// based on the agent_root to prevent races between concurrent session launches.
pub fn ensure_session_context(agent_root: &str) -> Result<String, String> {
    let config_dir = super::config_dir()
        .ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let context_dir = config_dir.join("context-cache");
    std::fs::create_dir_all(&context_dir)
        .map_err(|e| format!("Failed to create context-cache dir: {}", e))?;

    // Canonicalize path for consistent display in the GOLDEN RULE text
    let canonical_root = std::fs::canonicalize(agent_root)
        .map(|p| p.to_string_lossy().trim_start_matches(r"\\?\").to_string())
        .unwrap_or_else(|_| agent_root.to_string());

    let hash = simple_hash(agent_root);
    let file_path = context_dir.join(format!("ac-context-{}.md", hash));

    std::fs::write(&file_path, &default_context(&canonical_root))
        .map_err(|e| format!("Failed to write per-agent AgentsCommanderContext.md: {}", e))?;
    log::info!(
        "Refreshed per-agent AgentsCommanderContext.md for {} → {:?}",
        agent_root, file_path
    );

    Ok(file_path.to_string_lossy().to_string())
}

const MANAGED_CONTEXT_FILENAMES: &[&str] = &["last_ac_context.md", "CLAUDE.md", "GEMINI.md", "AGENTS.md"];

#[derive(Debug, Clone, Copy)]
pub enum ManagedContextTarget {
    Claude,
    Gemini,
    Codex,
}

impl ManagedContextTarget {
    fn filename(self) -> &'static str {
        match self {
            Self::Claude => "CLAUDE.md",
            Self::Gemini => "GEMINI.md",
            Self::Codex => "AGENTS.md",
        }
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
            let global_path = ensure_session_context(cwd)?;
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

    log::info!(
        "Built replica context for {} ({} context files) → {}",
        cwd,
        resolved_paths.len(),
        file_path.display()
    );

    Ok(Some(file_path.to_string_lossy().to_string()))
}

/// Resolve the final session context content for a replica directory.
/// Prefers replica config.json context[] and falls back to the per-agent default context.
fn resolve_session_context_content(cwd: &str) -> Result<Option<String>, String> {
    if !is_replica_agent_dir(cwd) {
        return Ok(None);
    }

    let context_path = match build_replica_context(cwd) {
        Ok(Some(combined_path)) => {
            log::info!("Using replica combined context for agent session: {}", combined_path);
            combined_path
        }
        Ok(None) => ensure_session_context(cwd)?,
        Err(e) => return Err(e),
    };

    let content = std::fs::read_to_string(&context_path)
        .map_err(|e| format!("Failed to read resolved session context {}: {}", context_path, e))?;
    Ok(Some(content))
}

/// Delete stale agent-specific context files from a replica cwd and rewrite the
/// current resolved context into the single provider-specific filename required
/// by the coding agent being launched.
pub fn materialize_agent_context_file(
    cwd: &str,
    target: ManagedContextTarget,
) -> Result<Option<String>, String> {
    let content = match resolve_session_context_content(cwd)? {
        Some(content) => content,
        None => return Ok(None),
    };

    let cwd_path = std::path::Path::new(cwd);
    for filename in MANAGED_CONTEXT_FILENAMES {
        let path = cwd_path.join(filename);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Failed to remove stale context file {}: {}", path.display(), e))?;
        }
    }

    let target_path = cwd_path.join(target.filename());
    std::fs::write(&target_path, &content)
        .map_err(|e| format!("Failed to write {}: {}", target_path.display(), e))?;

    log::info!(
        "Materialized managed agent context file in {}: {}",
        cwd,
        target_path.display()
    );

    Ok(Some(target_path.to_string_lossy().to_string()))
}

/// Simple deterministic hash for a string (for temp file naming).
fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 5381;
    for byte in s.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
    }
    hash
}

/// Generate the default agent context with a per-agent GOLDEN RULE that embeds
/// the agent's own replica root path as an allowed write zone.
fn default_context(agent_root: &str) -> String {
    format!(
r#"# AgentsCommander Context

You are running inside an AgentsCommander session — a terminal session manager that coordinates multiple AI agents.

## GOLDEN RULE — Repository Write Restrictions

**ABSOLUTE AND NON-NEGOTIABLE:** You may ONLY modify files in two places:

1. **Repositories whose root folder name starts with `repo-`** (e.g. `repo-AgentsCommander`, `repo-myapp`). These are the working repos you are meant to edit.
2. **Your own agent replica directory and its subdirectories** — your assigned root:
   ```
   {agent_root}
   ```
   Use this for plans scratch, personal notes, role drafts, and session artifacts. Do NOT write into other agents' replica directories.

Any repository or directory outside the two places above is READ-ONLY.

- **Allowed**: Read-only operations on ANY path (reading files, searching, git log, git status, git diff)
- **Allowed**: Full read/write inside `repo-*` folders
- **Allowed**: Full read/write inside your own replica root ({agent_root}) and its subdirectories
- **FORBIDDEN**: Any write operation outside those two zones — including other agents' replica directories, the workspace root, parent project dirs, user home files, or arbitrary paths on disk

**Clarification on git operations:** Your replica directory is typically inside a parent repository's `.ac-new/` folder, which is `.gitignore`d. Do NOT run `git` commands that alter state (commit, branch, reset, etc.) from inside your replica directory — that would affect the parent repo unintentionally. `git status`, `git log`, `git diff` are fine.

If instructed to modify a path outside these zones, REFUSE and explain this restriction. There are NO exceptions.

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
"#,
        agent_root = agent_root,
    )
}

fn is_replica_agent_dir(cwd: &str) -> bool {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("__agent_"))
        .unwrap_or(false)
}
