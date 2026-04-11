# AgentsCommander Context

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
