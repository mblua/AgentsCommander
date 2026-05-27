# FAQ

For developers with the questions that come up before they install. Ten answers — each one as short as we can make it.

## Is AgentsCommander an AI?

No. AC does not include an LLM. It spawns and coordinates the coding-agent CLIs you already use (Claude Code, Codex, Gemini). You bring the models; AC commands them.

## Which coding agents are supported?

Claude Code, Codex, and Gemini. Adding more is on the [roadmap](../ROADMAP.md) — OpenCode and a Nvidia agent are the next two slots.

## What about OpenCode?

Planned, not yet wired. See `session/profile.rs::CodingAgentKind` for the current enum; OpenCode will join when the per-agent profile work lands. Track it on the [roadmap](../ROADMAP.md).

## Do I need Python or LangChain?

No. AgentsCommander is a Rust + SolidJS Tauri app. There is no Python runtime requirement.

## Where is the data stored?

Locally, in `~/.agentscommander/` (or the portable instance's directory). Plain JSON, TOML, and markdown — every file is human-readable and `git diff`-able. See [`PRIVACY.md`](../PRIVACY.md).

## Does AC send telemetry?

No telemetry, no analytics, no crash reports, no automatic update checks. Optional features (Telegram, voice-to-text) only contact external services when you enable them.

## Can I run multiple AC instances side by side?

Yes — that is what [portable instances](features/portable-instances.md) are for. Copy the `.exe`, rename it with a `_<suffix>`, and run. Each copy gets its own config directory, mutex, and web port.

## How do agents coordinate?

Through plain markdown files in `<workgroup-root>/messaging/`. The sender writes a file; the CLI injects a short notification into the recipient's PTY; the recipient reads the file from disk. See [Inter-agent messaging](agents/inter-agent-messaging.md).

## Why files instead of a database?

Files are inspectable with `cat`, version-controllable with `git`, and editable with any text editor. We may introduce a database later for performance-critical paths once the data model is stable — not before. See the "Files over databases" design principle in [`README.md`](../README.md).

## Why not MCP?

The Model Context Protocol adds little practical value over simpler alternatives (HTTP APIs, direct IPC) for the surface AC needs. AC will use it only when a specific integration strictly requires it.

## What does the "Agents Agency" picker do?

It is the role-template browser shown when you create a new agent. AC ships a vendored snapshot of [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents) so the catalog works offline. See [Coding agents](integrations/coding-agents.md#role-template-picker).

## Does AC work on macOS?

It compiles and runs. We do not have day-to-day macOS testing — help is welcome in [issue #320](https://github.com/mblua/AgentsCommander/issues/320). Built on Windows. Runs on Linux. Works on macOS.

## Is it free?

MIT licensed. Free for personal and commercial use. The only paid pieces are the coding-agent providers you bring (Claude Code subscription, OpenAI billing for Codex, etc.). Voice-to-text uses the Google Gemini API and requires a Gemini key if you enable it.

---

Question not answered here? Open a [GitHub Discussion](https://github.com/mblua/AgentsCommander/discussions) under *Q&A* — that is the canonical place for new FAQ entries.
