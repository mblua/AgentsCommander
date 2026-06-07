# Roadmap

This is what's next for AgentsCommander. Items are grouped by status: **Shipped** / **Planned** / **Considering**. Order within each section is rough priority.

This file is a snapshot. The authoritative status for any item lives in its linked GitHub issue.

## Shipped (highlights)

- Multi-agent workgroups with file-based inter-agent messaging
- Cross-coding-agent profiles: Claude Code · Codex · Gemini
- Agents Agency role-template picker (explicit downloaded cache from [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents))
- Telegram bridge (PTY-mode + JSONL reader-mode per agent, plus photo/image sends for screenshots and artifacts)
- Voice-to-text via Gemini API with auto-execute
- Portable instances (rename the `.exe` → isolated workspace, ports, mutex, config)
- Embedded HTTP/WebSocket server (per-instance, opt-in)
- Step-by-step PTY visibility per agent (xterm.js + WebGL + ConPTY)
- RTK token-savings hook injection for Claude Code (auto-detect on PATH at startup)

## Planned

### Coding-agent integrations

- **OpenCode** - first-class profile alongside Claude Code, Codex, Gemini. ([#315](https://github.com/mblua/AgentsCommander/issues/315))
- **NVIDIA OpenShell** - sandbox-runtime integration (not a coding-agent profile; OpenShell hosts other coding agents inside a policy-driven sandbox). ([#316](https://github.com/mblua/AgentsCommander/issues/316))

### Per-agent configuration

- **Per-agent model + effort-level selection** - pin a specific model (e.g. Opus 4.7 vs Sonnet 4.6) and a thinking-effort tier (low / medium / high / ultra) to each agent independently. Today the choice is per-session at launch time; this would persist per `_agent_<name>` in the matrix and survive across workgroups. ([#329](https://github.com/mblua/AgentsCommander/issues/329))

### Organigram & scaling

- **Build the company - multi-level coordinator hierarchies** - A workgroup today is one layer. The next step layers them: level-A coordinator talks to its level-B reports; B talks to C; A never reaches C directly. Skip-levels flow through the chain, like in any sane org. The result: a real organigram, enforced by the messaging topology instead of by convention. Cascade decisions down, roll status up, hold accountability at 5, 50, or 500 agents the same way. ([#330](https://github.com/mblua/AgentsCommander/issues/330))
- **Curated role-team library** - ship a base catalog of task-ready teams, not just isolated roles. Each template defines the coordinator, specialist roles, task decomposition pattern, role boundaries, quality gates, and handoff contracts so a user can delegate a goal to a pre-curated team that can execute end to end with predictable accountability.
- **Coordinator auto-handoff for context management** - when a coordinator reaches a configurable context threshold, it can write a compact handoff of active continuity state and continue fresh. Forgettable or exclude-from-handoff details stay out. ([#349](https://github.com/mblua/AgentsCommander/issues/349))
- **Telegram conversation routing between coordinators** - transfer a Telegram conversation from one coordinator/workgroup to another without manual bot or channel reconfiguration. ([#350](https://github.com/mblua/AgentsCommander/issues/350))

### CLI capabilities

- **`create-workgroup` / `create-team` / `create-project`** - non-interactive equivalents of the existing `create-agent` flow for headless / script-driven setup. ([#317](https://github.com/mblua/AgentsCommander/issues/317))

### Automation

- **Cron-based scheduled executions** - define explicit, inspectable, and auditable cron schedules that regularly trigger events for a coordinator, workgroup, agent, or workflow target, supporting recurring status checks, reports, maintenance tasks, and scheduled workgroup runs. ([#354](https://github.com/mblua/AgentsCommander/issues/354))

### Execution determinism

- **AC Harness** - deterministic command-execution layer that complements RTK, plus extending RTK compatibility beyond Claude to Codex, Gemini, and future agents (today only Claude's `.claude/settings.local.json` hook is wired). ([#318](https://github.com/mblua/AgentsCommander/issues/318))

### Interoperability

- **Export AC agent configs** to **LangChain Deep Agents**, **OpenAI Agents SDK**, and **CrewAI** - author an agent once in AC, run it programmatically anywhere. ([#319](https://github.com/mblua/AgentsCommander/issues/319))

### Platform

- **macOS first-class verification** - testers wanted. ([#320](https://github.com/mblua/AgentsCommander/issues/320))
- **Web landing page** on `agentscommander.dev` or GitHub Pages from `/docs`. ([#321](https://github.com/mblua/AgentsCommander/issues/321))

## Considering

- **Discord community** - defer until star count justifies it (~500 stars). ([#322](https://github.com/mblua/AgentsCommander/issues/322))
- `docs/recipes/` - end-to-end multi-agent walkthroughs
- **`cargo udeps` cleanup** - audit and trim unused crates (start with `futures` vs `futures-util`). ([#323](https://github.com/mblua/AgentsCommander/issues/323))

---

Have an idea? Open a GitHub Discussion under **Ideas** or comment on the relevant issue above.
