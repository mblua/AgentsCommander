# Coding agents

For developers configuring which coding-agent CLIs AgentsCommander launches and how. Covers Claude Code, Codex, Gemini, the Agents Agency role-template picker, and adding your own custom agent.

AgentsCommander is **not** a coding agent. It spawns coding-agent processes and routes between them. You bring the CLIs; AC commands them.

## Supported coding agents

| Coding agent | Binary | Resume tokens AC injects | Notes |
|---|---|---|---|
| **Claude Code** | `claude` (or wrappers like `claude-mb`) | `--continue` | Anthropic's official CLI. |
| **Codex** | `codex` | `resume --last` | OpenAI's coding agent CLI. |
| **Gemini** | `gemini` | `--resume latest` | Google's CLI. |

> **OpenCode** runs today through the custom coding-agent path (see [Adding a custom coding agent](#adding-a-custom-coding-agent)). OpenCode is provider-agnostic, so you can point it at any provider or model, including OpenRouter Fusion. It does not yet have a first-class tuned integration (resume tokens, idle tuning); that work is tracked as [#315](https://github.com/mblua/AgentsCommander/issues/315).

Detection rule: AC inspects the shell command + args, takes each token's executable basename (lowercased, `.exe` stripped), and matches by **prefix** with precedence Claude > Codex > Gemini. Wrappers named `claude-foo` or `codex-bar` match automatically. Anything else falls through to the plain-shell behavior (no resume tokens, generic idle tuning).

The full enum is in `session/profile.rs::CodingAgentKind`.

## Installing the CLIs

AC does not install the coding-agent binaries. Use the upstream installers:

- **Claude Code** — [docs.claude.com/en/docs/claude-code](https://docs.claude.com/en/docs/claude-code)
- **Codex** — [github.com/openai/codex](https://github.com/openai/codex)
- **Gemini** — [github.com/google-gemini/gemini-cli](https://github.com/google-gemini/gemini-cli)

After install, each CLI authenticates itself (login flow, API key, or both). Under the local-process runtime AC does not touch those credentials. Under the **Container** runtime AC copies your host credential file into the container by default: see [Container coding agents](../features/container-coding-agents.md).

## How AC finds them

On startup AC reads `settings.json → agents[]`. Each entry has:

```json
{
  "id": "claude",
  "label": "Claude Code",
  "command": "claude",
  "color": "#E87B35"
}
```

| Field | Meaning |
|---|---|
| `id` | Stable internal id used by `create-agent --launch <id>`. |
| `label` | Display name in the launcher dropdown. |
| `command` | The binary to spawn. Resolved against `PATH` unless absolute. |
| `color` | Sidebar accent color for sessions launched with this agent. |

The default `settings.json` ships with one entry per supported agent.

## Switching the coding agent per session

When you launch a session AC shows a dropdown listing every entry in `agents[]`. Pick one. The choice is remembered as the session's `lastCodingAgent` so subsequent wakeups use the same CLI without asking.

You can change the choice for a session later: right-click → **Launch with…** → pick a different agent.

## Profiles: launch variants per coding agent

Each coding agent can have several **profiles** (lettered launch variants: a cheap one, a max-effort one, an isolated-config one). A profile adds parameters and env vars on top of the agent's base command, and you assign one per agent or per session. This is a separate feature from the tuned `CodingAgentKind` integration above. See [Coding Agent Profiles](../features/coding-agent-profiles.md).

## Role-template picker

When you create a new agent through the UI you can pick a role template. The picker shows two sources:

1. **Agency templates** — read from the validated offline cache at `<config-dir>/agency-agents_templates`, refreshed only by `agency-templates update`.
2. **Local templates** — read from `<config-dir>/agent-templates/<folder>/` (override the path via `settings.agentTemplatesPath`).

Each template provides metadata (name, description, category, accent color) and a markdown role body. AC writes the body into the new agent's `Role.md` and `CLAUDE.md`.

> AC's role-template picker can use a downloaded cache of [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents). If you author a new role and want it discoverable in AC by default, submit it upstream to the agency-agents catalog, then refresh the cache with `agency-templates update`.

## Adding a custom coding agent

To make AC recognise a new CLI (e.g. a custom wrapper) under the **Coding Agents** dropdown:

1. Open **Settings → Coding Agents → Add agent**.
2. Fill in `id`, `label`, `command`, and accent `color`.
3. Save.

The new entry appears in the launcher dropdown immediately. AC will spawn the binary as-is — no resume tokens are injected unless the binary's basename starts with `claude`, `codex`, or `gemini`.

For deeper integration (a new `CodingAgentKind` with its own resume tokens and idle tuning), you need to add a variant to `src-tauri/src/session/profile.rs` and rebuild. OpenCode already runs through the custom-agent steps above; its first-class tuned integration is tracked on the [roadmap](../../ROADMAP.md) ([#315](https://github.com/mblua/AgentsCommander/issues/315)) as the canonical example of how a new `CodingAgentKind` is added.

> **"Profile" here does not mean the profile matrix.** A tuned `CodingAgentKind` is how AC drives one CLI (resume tokens, idle tuning). The lettered launch variants (A/B/C) are a separate feature: see [Coding Agent Profiles](../features/coding-agent-profiles.md).

## Authentication and secrets

AC does not store coding-agent credentials of its own. Each CLI manages its own, on the host:

| CLI | Where it stores credentials |
|---|---|
| Claude Code | `~/.claude/` |
| Codex | `~/.codex/` |
| Gemini | `~/.gemini/` |

Under the **local-process** runtime, AC never copies or writes those credentials: the files it writes into agent directories are configuration only.

Under the **Container** runtime there is one deliberate exception, on by default. AC copies the host user's credential file for that coding agent (Claude: `~/.claude/.credentials.json`) into the replica config dir so the container starts signed in, and deletes it when the session stops. It is Claude Code only today, it puts a full-account token in plaintext inside the workspace tree, and it is governed by the `containerCredentialsFromHost` setting. Read [Container coding agents](../features/container-coding-agents.md) and [Security model](../security.md#container-coding-agents-copied-host-credentials) before you rely on it.

## See also

- [Coding Agent Profiles](../features/coding-agent-profiles.md): lettered launch variants (A/B/C) per coding agent
- [Container coding agents](../features/container-coding-agents.md): host login reuse, and why container agents cannot reach repos yet
- [Creating agents](../agents/creating-agents.md) — make a new agent dir
- [Settings reference](../reference/settings.md) — full schema for `agents[]`
- [Roadmap: coding agents](../../ROADMAP.md): OpenCode first-class integration, Nvidia agent, more
