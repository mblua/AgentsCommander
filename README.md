# Agents Commander

The command center for AI coding agents.

- Run Claude Code, Codex, and OpenCode in parallel
- Instant idle detection - know which agent needs you
- Dictate prompts by voice
- Detach sessions into independent windows
- Built-in tips to improve your coding agent workflow

Built with **Tauri 2.x** (Rust) + **SolidJS** (TypeScript) + **xterm.js** (WebGL).

## Features

- **Agent launcher** - Run Claude Code, Codex, OpenCode, and other CLI agents from a single dashboard
- **Idle detection** - Visual indicator (green dot) when an agent is done and waiting for you
- **Voice-to-text** - Dictate prompts via Gemini transcription (push-to-talk in terminal, toggle in sidebar) with auto-execute and cancel support
- **Detached windows** - Pop any session out into its own dedicated terminal window
- **Best practices hints** - Contextual tips to sharpen your coding agent workflow
- **Decoupled multi-window** - Sidebar and Terminal are independent windows, not tabs in a single frame
- **Full PTY emulation** - Real terminal via ConPTY (portable-pty), not a command runner
- **xterm.js with WebGL** - Hardware-accelerated rendering with canvas fallback
- **Session management** - Create, rename, switch, destroy sessions from the sidebar
- **Team filter** - Filter sessions by team in the sidebar dropdown
- **Telegram bridge** - Attach a Telegram bot to a session for remote monitoring
- **Settings UI** - Tabbed settings modal (General, Coding Agents, Integrations, Dark Factory) accessible from the top bar
- **Zoom support** - Ctrl+Scroll, Ctrl++/-, Ctrl+0 on any window, with per-window zoom level persistence
- **Window geometry persistence** - Windows reopen at the same position and size as when you last closed the app
- **Keyboard shortcuts** - New session, close, switch, voice toggle (Ctrl+Shift+R)
- **Configurable** - Shell, args, repo paths, agents, and bots via `settings.json` (next to binary)
- **Portable instances** - Copy the exe, rename with a suffix, run. Each copy is fully isolated with its own config

## Design Principles

These are deliberate choices that shape the project. They are not accidents.

- **No MCP.** We consider the Model Context Protocol to add little practical value over simpler alternatives (HTTP APIs, direct IPC). It is not used unless a specific integration strictly requires it.

- **Files over databases.** All state and communication is persisted to plain files (JSON, TOML). This makes every change visible via `git diff`, trivial to inspect, and easy to debug. Databases will be introduced later for performance-critical paths once the data model is mature - not before.

- **One agent = one directory.** An agent is defined by a `CLAUDE.md` file (or equivalent role prompt file) inside its own directory. A directory can optionally include a `.agentscommander/` config folder to specify a custom role prompt file path (e.g., if it is not named `CLAUDE.md`), but the file must still live within the root of that directory. Multiple role prompts within the same directory or its subdirectories are strictly forbidden. Why? Most coding agents assume that the entire contents of their working directory are relevant context and may read files freely. If multiple role prompts coexisted in one directory tree, an agent could inadvertently read another agent's role - leaking context, confusing behavior, and wasting tokens. To run multiple agents from a single repository, structure it so each agent has its own subdirectory with its own `CLAUDE.md` inside.

## Platform Support

| Platform | Status |
|----------|--------|
| Windows | Tested - primary development platform |
| Linux | Compatible - less tested |
| macOS | Compatible - not tested |

## Tech Stack

| Layer | Tech |
|-------|------|
| App framework | Tauri 2.x |
| Backend | Rust + tokio |
| Frontend | SolidJS + TypeScript |
| Terminal | xterm.js (WebGL addon) |
| PTY | portable-pty (ConPTY on Windows, Unix PTY on Linux/macOS) |
| Styles | Vanilla CSS + CSS variables |
| Bundler | Vite 6 |

## Prerequisites

- [Node.js](https://nodejs.org/) 20+
- [Rust](https://rustup.rs/) (stable)
- **Windows**: Windows 10 1809+ (ConPTY support required)
- **Linux**: `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`
- **macOS**: Xcode Command Line Tools

## Development

```bash
# Install frontend dependencies
npm install

# Run in dev mode (hot reload)
npm run tauri dev

# Kill stale dev instances (safe - only kills target\debug)
npm run kill-dev
```

### Checks

```bash
# TypeScript
npx tsc --noEmit

# Rust
cd src-tauri && cargo check
cd src-tauri && cargo clippy
cd src-tauri && cargo test
```

## Build

```bash
npm run tauri build
```

The production binary is at `src-tauri/target/release/agentscommander` (`.exe` on Windows). Run it directly - do not use the NSIS/MSI installers for local testing.

## Releases

Releases are automated via GitHub Actions. Push a tag to trigger a build:

```bash
git tag v0.4.9
git push origin v0.4.9
```

This creates a draft release with auto-generated changelog and installers for Windows, macOS (ARM + Intel), and Ubuntu.

## Configuration

Settings are stored in a `.agentscommander*/settings.json` file next to the binary (see [Portable Instances](#portable-instances) below).

On Windows the default shell is `powershell.exe`; on Linux/macOS it is `/bin/bash`.

```json
{
  "defaultShell": "powershell.exe",
  "defaultShellArgs": ["-NoLogo"],
  "repoPaths": ["C:/repos"],
  "agents": [
    {
      "id": "claude",
      "label": "Claude Code",
      "command": "claude",
      "color": "#E87B35",
      "gitPullBefore": false
    }
  ],
  "sidebarAlwaysOnTop": true,
  "raiseTerminalOnClick": true,
  "voiceToTextEnabled": false,
  "geminiApiKey": "",
  "geminiModel": "gemini-2.5-flash",
  "voiceAutoExecute": true,
  "voiceAutoExecuteDelay": 15,
  "sidebarZoom": 1.0,
  "terminalZoom": 1.0,
  "sidebarGeometry": null,
  "terminalGeometry": null
}
```

## Portable Instances

Agents Commander is fully portable. The binary carries everything it needs — no installation required.

### Config directory

The config directory lives **next to the binary**, named after it:

```
C:\tools\agentscommander.exe          -> C:\tools\.agentscommander\
C:\tools\agentscommander_stage.exe    -> C:\tools\.agentscommander_stage\
C:\work\agentscommander_team-a.exe    -> C:\work\.agentscommander_team-a\
```

Each config directory contains `settings.json`, `sessions.json`, web tokens, and all instance state. Two copies of the binary in different folders (or with different names) are **completely independent** — separate settings, sessions, ports, and mutex.

### Instance labeling

Rename the binary with an underscore suffix to create a labeled instance:

```
agentscommander_<suffix>.exe
```

The suffix (uppercased) appears as a badge in the titlebar and affects isolation:

| Binary name | Titlebar | Mutex | Web port |
|---|---|---|---|
| `agentscommander.exe` | Agents Commander | Shared (prod) | 9877 |
| `agentscommander_stage.exe` | Agents Commander **[STAGE]** | Unique | 9878 |
| `agentscommander_dev.exe` | Agents Commander **[DEV]** | Unique | 9876 |
| `agentscommander_team-a.exe` | Agents Commander **[TEAM-A]** | Unique | Auto (9880-9899) |

Unknown suffixes get a deterministic port in the 9880-9899 range based on a hash of the suffix name.

### Creating a new isolated instance

1. Copy `agentscommander.exe` to any folder
2. Rename it with an underscore suffix: `agentscommander_myteam.exe`
3. Run it

That's it. The instance creates its own config directory on first launch, gets a unique mutex (so it won't conflict with other instances), and shows **[MYTEAM]** in the titlebar.

## CLI

The `agentscommander` binary doubles as a CLI for agent-to-agent operations. Available subcommands:

### `send` — Send a message to another agent

```bash
# Send a file-based message (two steps: write the file, then send)
#   1. Write the message content to <workgroup-root>/messaging/YYYYMMDD-HHMMSS-<wgN>-<from>-to-<wgN>-<to>-<slug>.md
#   2. Fire the send:
agentscommander send --token <TOKEN> --root <CWD> --to <agent_name> --send <filename> --mode wake

# Send a remote command (clear or compact)
agentscommander send --token <TOKEN> --root <CWD> --to <agent_name> --command clear --mode wake
```

All messages are delivered synchronously — the CLI validates routing, delivers, and confirms before exiting. There is no background queue.

| Flag | Required | Description |
|------|----------|-------------|
| `--token` | No | Session token for authentication |
| `--root` | Yes | Sender's root directory (must be under a `wg-<N>-*` ancestor for `--send`) |
| `--to` | Yes | Destination agent name (e.g., `"repos/my-project"`) |
| `--send` | No* | Filename (not path) of a message file already written in `<workgroup-root>/messaging/` |
| `--command` | No* | Remote command to execute (whitelist: `clear`, `compact`) |
| `--mode` | No | Delivery mode: `wake` (default and currently the only supported value; reserved for future modes) |
| `--get-output` | No | Wait for and return the agent's response. **Currently non-functional under `--mode wake` (the only supported mode); reserved for future reimplementation.** |
| `--timeout` | No | Timeout in seconds for `--get-output` (default: 300) |

*Exactly one of `--send` or `--command` is required. They are mutually exclusive.

**File-based messaging** is the only message-delivery mechanism. The CLI injects a short notification into the recipient's PTY pointing at the file's absolute path; the recipient reads the content via filesystem, bypassing any PTY truncation regardless of payload size. Senders write the file first (UTC-timestamped filename following the canonical shape `YYYYMMDD-HHMMSS-<wgN>-<from>-to-<wgN>-<to>-<slug>[.N].md`, sanitized kebab-case slug ≤50 chars), then invoke `--send <filename>`. The file persists in `<workgroup-root>/messaging/` — it is never auto-purged. Note: filenames use UTC, so the timestamp on a non-UTC host will differ from the local wall clock.

**Remote commands** (`--command`) inject a slash command (e.g. `/clear`) directly into the agent's PTY. The destination agent must be idle (green circle in the sidebar) — the command is rejected otherwise.

**Delivery modes:**
- `wake` — Inject into the recipient's PTY. If an active session exists, the message is written to stdin regardless of whether the agent is mid-turn (the stdin buffer absorbs it; the agent reads on the next idle). If the session is Exited it is destroyed and a fresh persistent one is spawned. If no session exists one is spawned.

**Exit codes:** `0` = message delivered and confirmed, `1` = routing rejected, delivery failed, or timeout.

**Pre-validation:** Before delivery, the CLI validates that the sender can reach the destination based on team membership and coordinator rules (`teams.json`). If routing would reject the message, the CLI fails immediately without writing to the outbox.

### `list-peers` — List available peers

```bash
agentscommander list-peers --token <TOKEN> --root <CWD>
```

### `create-agent` — Create a new agent

Creates a folder with a `CLAUDE.md` role prompt. Optionally launches it with a coding agent.

```bash
# Create only
agentscommander create-agent --parent "C:\path\to\folder" --name "MyAgent"

# Create and launch with Claude Code
agentscommander create-agent --parent "C:\path\to\folder" --name "MyAgent" --launch claude
```

| Flag | Required | Description |
|------|----------|-------------|
| `--parent` | Yes | Parent directory where the agent folder will be created |
| `--name` | Yes | Agent name (becomes a subfolder inside `--parent`) |
| `--launch` | No | Coding agent id to launch after creation (e.g., `claude`, `codex`) |
| `--root` | No | Caller's root directory (for context) |
| `--token` | No | Session token (for auth context) |

**What it does:**
1. Creates `<parent>/<name>/` directory
2. Writes `CLAUDE.md` with content: `You are the agent <parentFolder>/<name>`
3. If `--launch` is provided, writes a session request that the running app picks up and launches automatically (~3s)

**Output** (stdout, JSON):
```json
{
  "agentPath": "C:\\path\\to\\folder\\MyAgent",
  "agentName": "folder/MyAgent",
  "claudeMd": "You are the agent folder/MyAgent",
  "launched": true,
  "launchAgent": "claude"
}
```

## Architecture

```
User types in xterm.js
  -> Tauri Command "pty_write(bytes)"
  -> Rust writes to PTY stdin

PTY stdout produces output
  -> Rust async read loop (tokio)
  -> Tauri Event "pty_output" { sessionId, data }
  -> xterm.js terminal.write(data)
```

Both windows share the same frontend bundle, differentiated by query param (`?window=sidebar` vs `?window=terminal`). IPC goes through typed wrappers in `src/shared/ipc.ts` - components never call `invoke()` directly.

## Project Structure

```
agentscommander/
├── src-tauri/src/
│   ├── lib.rs               # App setup, multi-window creation
│   ├── commands/             # Tauri IPC handlers (session, pty, config, window, telegram)
│   ├── session/              # SessionManager + Session struct
│   ├── pty/                  # PtyManager + IdleDetector
│   ├── config/               # AppSettings (load/save)
│   ├── telegram/             # Telegram bridge (manager, bridge, API)
│   └── errors.rs             # AppError enum (thiserror)
├── src/
│   ├── sidebar/              # Sidebar window (session list, toolbar, settings)
│   ├── terminal/             # Terminal window (xterm.js, status bar)
│   └── shared/               # Types, IPC wrappers, shortcuts, constants
├── scripts/
│   └── kill-dev.ps1          # Safe dev-instance killer
└── .github/workflows/
    └── release.yml           # CI: build + draft release on tag push
```

## Version

Current: **0.4.8**

Version is kept in sync across three files:
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`
- `src/sidebar/components/Titlebar.tsx`

## Privacy

Agents Commander does not collect telemetry, analytics, or usage data. Optional features (Telegram Bridge, Voice-to-Text) transmit data to external services only when explicitly enabled by the user. See [PRIVACY.md](PRIVACY.md) for details.

## Code Signing

Windows releases are digitally signed. See [Code Signing Policy](CODE_SIGNING_POLICY.md).

Free code signing provided by [SignPath.io](https://signpath.io), certificate by [SignPath Foundation](https://signpath.org).

## Author

**Mariano Blua** — [GitHub](https://github.com/mblua) · [LinkedIn](https://www.linkedin.com/in/mariano-blua)

## License

[MIT](LICENSE)
