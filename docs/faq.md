# FAQ

For developers with the questions that come up before they install. Thirteen answers — each one as short as we can make it.

## Is AgentsCommander an AI?

No. AC does not include an LLM. It spawns and coordinates the coding-agent CLIs you already use (Claude Code, Codex, Antigravity, and Pi). You bring the models; AC commands them.

## Which coding agents are supported?

Claude Code, Codex, Antigravity, and Pi have first-class tuned integrations. Pi uses exact command-position detection and continues eligible known-state conversations with `--continue`; see [Coding agents](integrations/coding-agents.md#pi-resume-behavior). OpenCode works today through the separate custom coding-agent path, where you can point it at any provider or model. A first-class OpenCode integration and an Nvidia agent are on the [roadmap](../ROADMAP.md).

## What about OpenCode?

Usable today. OpenCode is provider-agnostic, so you add it as a custom coding agent (Settings, Coding Agents, Add agent) and point it at any provider or model. That is what lets AC drive any model through it, including OpenRouter Fusion. What is still planned is a first-class tuned integration: its own `CodingAgentKind` variant with resume tokens and idle tuning. The current enum is in `session/profile.rs::CodingAgentKind`. Track the tuned integration on the [roadmap](../ROADMAP.md).

## Do I need Python or LangChain?

No. AgentsCommander is a Rust + SolidJS Tauri app. There is no Python runtime requirement.

## Where is the data stored?

Locally, in the per-instance config directory next to the binary (for example `C:\tools\.agentscommander\` for `C:\tools\agentscommander.exe`), with a legacy `$HOME` fallback when the executable path is unavailable. Projects keep their shared state in their own `.ac/` folder. Plain JSON, TOML, and markdown — every file is human-readable and `git diff`-able. See [Portable instances](features/portable-instances.md) and [`PRIVACY.md`](../PRIVACY.md).

## Does AC send telemetry?

No telemetry, no analytics, no crash reports. The one automatic network check is the npm update check: on startup AC queries the npm registry for the latest published version (throttled to at most once per 24 hours, fail-silent) and shows an in-app notice when a newer version exists. No user data leaves the machine; the query is a plain version lookup. You can turn it off with `npmUpdateNotificationsEnabled: false` in `settings.json`. Optional features (Telegram, voice-to-text) only contact external services when you enable them.

## Can I run multiple AC instances side by side?

Yes — that is what [portable instances](features/portable-instances.md) are for. Copy the `.exe`, rename it with a `_<suffix>`, and run. Each copy gets its own config directory, mutex, and web port.

## How do agents coordinate?

Through plain markdown files in `<room-root>/messaging/`. The sender writes a file; the CLI injects a short notification into the recipient's PTY; the recipient reads the file from disk. See [Inter-agent messaging](agents/inter-agent-messaging.md).

## Why files instead of a database?

Files are inspectable with `cat`, version-controllable with `git`, and editable with any text editor. We may introduce a database later for performance-critical paths once the data model is stable — not before. See the "Files over databases" design principle in [`README.md`](../README.md).

## Why not MCP?

The Model Context Protocol adds little practical value over simpler alternatives (HTTP APIs, direct IPC) for the surface AC needs. AC will use it only when a specific integration strictly requires it.

## What does the "Agents Agency" picker do?

It is the role-template browser shown when you create a new agent. AC can download a validated cache of [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents) so the catalog works offline after an explicit `agency-templates update`. See [Coding agents](integrations/coding-agents.md#role-template-picker).

## Does AC work on macOS?

macOS is not supported yet because maintainer and test capacity is insufficient. A release artifact is not a support promise, so a normal install stops on macOS. Windows 10 1809+ and Windows 11 on x86_64/AMD64 are fully supported; Linux x86_64/AMD64 support is partial and in progress. See the [platform contract](install-with-agent.md#support-gates). If you deliberately test Linux or macOS, use the [reproducible report template](install-with-agent.md#help-extend-linux-and-macos-support) or follow [`CONTRIBUTING.md`](../CONTRIBUTING.md).

## Is it free?

MIT licensed. Free for personal and commercial use. The only paid pieces are the coding-agent providers you bring (Claude Code subscription, OpenAI billing for Codex, etc.). Voice-to-text uses the Google Gemini API and requires a Gemini key if you enable it.

---

Question not answered here? Open a [GitHub Discussion](https://github.com/mblua/AgentsCommander/discussions) under *Q&A* — that is the canonical place for new FAQ entries.
