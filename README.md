# SummonGate

A standalone Windows terminal session manager with decoupled tabs. Two synchronized windows work together: a narrow **Sidebar** for managing sessions, and a full **Terminal** window rendering the active PTY via xterm.js.

Built with **Tauri 2.x** (Rust) + **SolidJS** (TypeScript) + **xterm.js** (WebGL).

## Features

- **Decoupled multi-window** - Sidebar and Terminal are independent windows, not tabs in a single frame
- **Full PTY emulation** - Real terminal via ConPTY (portable-pty), not a command runner
- **xterm.js with WebGL** - Hardware-accelerated rendering with canvas fallback
- **Session management** - Create, rename, switch, destroy sessions from the sidebar
- **Detached windows** - Pop a session out into its own dedicated terminal window
- **Idle detection** - Visual indicator (green dot) when a session is idle vs busy
- **Agent launcher** - Open pre-configured CLI agents (Claude Code, etc.) from the toolbar
- **Telegram bridge** - Attach a Telegram bot to a session for remote monitoring
- **Custom titlebar** - Frameless windows with draggable titlebar, no native decorations
- **Keyboard shortcuts** - New session, close, switch between sessions
- **Configurable** - Shell, args, repo paths, agents, and bots via `~/.summongate/settings.json`

## Design Principles

These are deliberate choices that shape the project. They are not accidents.

- **No MCP.** We consider the Model Context Protocol to add little practical value over simpler alternatives (HTTP APIs, direct IPC). It is not used unless a specific integration strictly requires it.

- **Files over databases.** All state and communication is persisted to plain files (JSON, TOML). This makes every change visible via `git diff`, trivial to inspect, and easy to debug. Databases will be introduced later for performance-critical paths once the data model is mature - not before.

- **One agent = one directory.** An agent is defined by a `CLAUDE.md` file (or equivalent role prompt file) inside its own directory. A directory can optionally include a `.agentscommander/` config folder to specify a custom role prompt file path (e.g., if it is not named `CLAUDE.md`), but the file must still live within the root of that directory. Multiple role prompts within the same directory or its subdirectories are strictly forbidden. Why? Most coding agents assume that the entire contents of their working directory are relevant context and may read files freely. If multiple role prompts coexisted in one directory tree, an agent could inadvertently read another agent's role - leaking context, confusing behavior, and wasting tokens. To run multiple agents from a single repository, structure it so each agent has its own subdirectory with its own `CLAUDE.md` inside.

## Tech Stack

| Layer | Tech |
|-------|------|
| App framework | Tauri 2.x |
| Backend | Rust + tokio |
| Frontend | SolidJS + TypeScript |
| Terminal | xterm.js (WebGL addon) |
| PTY | portable-pty (ConPTY on Windows) |
| Styles | Vanilla CSS + CSS variables |
| Bundler | Vite 6 |

## Prerequisites

- [Node.js](https://nodejs.org/) 20+
- [Rust](https://rustup.rs/) (stable)
- Windows 10 1809+ (ConPTY support required)

For Linux builds: `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`

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

The production binary is at `src-tauri/target/release/summongate.exe`. Run it directly - do not use the NSIS/MSI installers for local testing.

## Releases

Releases are automated via GitHub Actions. Push a tag to trigger a build:

```bash
git tag v0.4.0
git push origin v0.4.0
```

This creates a draft release with installers for Windows, macOS (ARM + Intel), and Ubuntu.

## Configuration

Settings are stored in `~/.summongate/settings.json`:

```json
{
  "defaultShell": "powershell.exe",
  "defaultShellArgs": ["-NoLogo"],
  "repoPaths": ["C:\\Users\\you\\repos"],
  "agents": [
    {
      "id": "claude",
      "label": "Claude Code",
      "command": "claude",
      "args": [],
      "color": "#E87B35"
    }
  ],
  "sidebarAlwaysOnTop": true,
  "raiseTerminalOnClick": true
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
summongate/
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

Current: **0.4.0**

Version is kept in sync across three files:
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`
- `src/sidebar/components/Titlebar.tsx`

## License

Private project.
