# Roadmap

This is what's next for AgentsCommander. Items are grouped by status: **Shipped** / **Planned** / **Considering**. Order within each section is rough priority.

This file is a snapshot. The authoritative status for any item lives in its linked GitHub issue.

## Shipped (highlights)

- Multi-agent workgroups with file-based inter-agent messaging
- Cross-coding-agent profiles: Claude Code · Codex · Gemini
- Agents Agency role-template picker (vendored snapshot of [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents))
- Telegram bridge (PTY-mode + JSONL reader-mode per agent)
- Voice-to-text via Gemini API with auto-execute
- Portable instances (rename the `.exe` → isolated workspace, ports, mutex, config)
- Embedded HTTP/WebSocket server (per-instance, opt-in)
- Step-by-step PTY visibility per agent (xterm.js + WebGL + ConPTY)
- RTK token-savings hook injection for Claude Code (auto-detect on PATH at startup)

## Planned

### Coding-agent integrations

- **OpenCode** — first-class profile alongside Claude Code, Codex, Gemini. ([#315](https://github.com/mblua/AgentsCommander/issues/315))
- **NVIDIA OpenShell** — sandbox-runtime integration (not a coding-agent profile; OpenShell hosts other coding agents inside a policy-driven sandbox). ([#316](https://github.com/mblua/AgentsCommander/issues/316))

### CLI capabilities

- **`create-workgroup` / `create-team` / `create-project`** — non-interactive equivalents of the existing `create-agent` flow for headless / script-driven setup. ([#317](https://github.com/mblua/AgentsCommander/issues/317))

### Execution determinism

- **AC Harness** — deterministic command-execution layer that complements RTK, plus extending RTK compatibility beyond Claude to Codex, Gemini, and future agents (today only Claude's `.claude/settings.local.json` hook is wired). ([#318](https://github.com/mblua/AgentsCommander/issues/318))

### Interoperability

- **Export AC agent configs** to **LangChain Deep Agents**, **OpenAI Agents SDK**, and **CrewAI** — author an agent once in AC, run it programmatically anywhere. ([#319](https://github.com/mblua/AgentsCommander/issues/319))

### Platform

- **macOS first-class verification** — testers wanted. ([#320](https://github.com/mblua/AgentsCommander/issues/320))
- **Web landing page** on `agentscommander.dev` or GitHub Pages from `/docs`. ([#321](https://github.com/mblua/AgentsCommander/issues/321))

## Considering

- **Discord community** — defer until star count justifies it (~500 stars). ([#322](https://github.com/mblua/AgentsCommander/issues/322))
- `docs/recipes/` — end-to-end multi-agent walkthroughs
- **`cargo udeps` cleanup** — audit and trim unused crates (start with `futures` vs `futures-util`). ([#323](https://github.com/mblua/AgentsCommander/issues/323))

---

Have an idea? Open a GitHub Discussion under **Ideas** or comment on the relevant issue above.
