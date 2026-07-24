# FAQ

For developers with the questions that come up before they install. Thirteen answers — each one as short as we can make it.

## Is AgentsCommander an AI?

No. AC does not include an LLM. It spawns and coordinates the coding-agent CLIs you already use (Claude Code, Codex, Gemini, and Pi). You bring the models; AC commands them.

## Which coding agents are supported?

Claude Code, Codex, Gemini, and Pi have first-class tuned integrations. Pi uses exact command-position detection and continues eligible known-state conversations with `--continue`; see [Coding agents](integrations/coding-agents.md#pi-resume-behavior). OpenCode works today through the separate custom coding-agent path, where you can point it at any provider or model. A first-class OpenCode integration and an Nvidia agent are on the [roadmap](../ROADMAP.md).

## What about OpenCode?

Usable today. OpenCode is provider-agnostic, so you add it as a custom coding agent (Settings, Coding Agents, Add agent) and point it at any provider or model. That is what lets AC drive any model through it, including OpenRouter Fusion. What is still planned is a first-class tuned integration: its own `CodingAgentKind` variant with resume tokens and idle tuning. The current enum is in `session/profile.rs::CodingAgentKind`. Track the tuned integration on the [roadmap](../ROADMAP.md).

## Do I need Python or LangChain?

No. AgentsCommander is a Rust + SolidJS Tauri app. There is no Python runtime requirement.

## Where is the data stored?

Locally. The canonical Linux DEB uses
`$XDG_CONFIG_HOME/agentscommander`, falling back to
`$HOME/.config/agentscommander`. Raw portable binaries use their
executable-relative config directory. The files are plain JSON, TOML, and
markdown. See [`PRIVACY.md`](../PRIVACY.md).

## Does AC send telemetry?

No telemetry, no analytics, no crash reports, no automatic update checks. Optional features (Telegram, voice-to-text) only contact external services when you enable them.

## Can I run multiple AC instances side by side?

Yes. Raw portable binaries with different config roots can run side by side;
renamed Windows copies are one example. On Linux, the lock is config-scoped:
two launches using the same canonical DEB config cannot run together, while
separate absolute XDG config roots can. See
[Portable instances](features/portable-instances.md).

## How do agents coordinate?

Through plain markdown files in `<workgroup-root>/messaging/`. The sender writes a file; the CLI injects a short notification into the recipient's PTY; the recipient reads the file from disk. See [Inter-agent messaging](agents/inter-agent-messaging.md).

## Why files instead of a database?

Files are inspectable with `cat`, version-controllable with `git`, and editable with any text editor. We may introduce a database later for performance-critical paths once the data model is stable — not before. See the "Files over databases" design principle in [`README.md`](../README.md).

## Why not MCP?

The Model Context Protocol adds little practical value over simpler alternatives (HTTP APIs, direct IPC) for the surface AC needs. AC will use it only when a specific integration strictly requires it.

## What does the "Agents Agency" picker do?

It is the role-template browser shown when you create a new agent. AC can download a validated cache of [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents) so the catalog works offline after an explicit `agency-templates update`. See [Coding agents](integrations/coding-agents.md#role-template-picker).

## Does AC work on macOS?

It compiles and runs. We do not have day-to-day macOS testing — help is welcome in [issue #320](https://github.com/mblua/AgentsCommander/issues/320). Built on Windows. Runs on Linux. Works on macOS.

## Is it free?

MIT licensed. Free for personal and commercial use. The only paid pieces are the coding-agent providers you bring (Claude Code subscription, OpenAI billing for Codex, etc.). Voice-to-text uses the Google Gemini API and requires a Gemini key if you enable it.

---

Question not answered here? Open a [GitHub Discussion](https://github.com/mblua/AgentsCommander/discussions) under *Q&A* — that is the canonical place for new FAQ entries.
