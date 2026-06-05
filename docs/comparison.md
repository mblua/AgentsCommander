# How AgentsCommander compares

For developers deciding between AgentsCommander and another multi-agent tool. Honest table; honest losses.

## At a glance

| | AgentsCommander | LangGraph | AutoGen / AG2 | CrewAI | Aider | Claude Code alone |
|---|---|---|---|---|---|---|
| **Operates real CLI coding agents** | ✅ Claude Code, Codex, Gemini | ❌ Python LLM calls | ❌ Python conversation | ❌ Python library | Partial (one agent) | ✅ (one agent) |
| **Real PTY per agent** | ✅ ConPTY / Unix PTY | ❌ | ❌ | ❌ | ✅ | ✅ |
| **Filesystem-first messaging** | ✅ Markdown in `messaging/` | ❌ DB / Python state | ❌ Python objects | ❌ Python tasks | n/a | n/a |
| **No Python / framework lock-in** | ✅ | ❌ | ❌ | ❌ | ❌ Python | ✅ |
| **Multi-agent on the same repo** | ✅ | Partial | Partial | Partial | ❌ | ❌ |
| **Desktop UI** | ✅ Tauri app | ❌ | ❌ | ❌ | TUI only | TUI only |
| **Voice-to-text in** | ✅ Gemini | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Telegram bridge** | ✅ Per-session | ❌ | ❌ | ❌ | ❌ | ❌ |

The table tells one story. The trade-offs below tell the rest.

## Versus LangGraph (LangChain)

LangGraph is a Python DSL for stateful agent workflows. You write the DAG; LangGraph drives the LLM calls.

**Where AC wins**:
- Zero framework lock-in. AC orchestrates Claude Code, Codex, and Gemini — the tools you already use day-to-day. No Python runtime required, no LangChain dependency.
- You watch real PTY output. There is no trace ID to copy into a separate observability dashboard.
- Files as the message bus. Every coordination step is `git diff`-able.

**Where LangGraph wins**:
- Massive ecosystem of integrations, retrievers, and provider adapters.
- Better fit when your agents are LLM calls embedded inside a Python backend.

## Versus AutoGen / AG2

AutoGen (and the community fork AG2) is Microsoft's multi-agent conversational framework. Agents are Python objects that exchange messages.

**Where AC wins**:
- Heterogeneous coding agents in the same workgroup. Claude Code on architecture, Codex on dev, Gemini on review — without writing a single adapter.
- Visible, observable, no Python runtime.

**Where AutoGen wins**:
- Larger research community and more academic citations.
- Conversation patterns (planner/critic, group chat, etc.) are well documented and battle-tested.

## Versus CrewAI

CrewAI defines role-based agent teams with hierarchical processes — in Python.

**Where AC wins**:
- Installs from npm, with desktop and portable installers still available from GitHub Releases. You can demo it in 60 seconds.
- No `pip install`, no config-by-code, no rewriting your tools as CrewAI primitives.

**Where CrewAI wins**:
- Easier to embed inside a backend service when agents are LLM calls.
- LangChain-style abstractions that some developers already know.

## Versus Aider

Aider is a TUI coding assistant that edits files via diffs.

**Where AC wins**:
- Multiple agents on the same repo, each with its own session.
- Cross-coding-agent: Aider has its own model picker, but it is one agent at a time.

**Where Aider wins**:
- More polished single-agent UX for solo work.
- Strong git-diff editing model — Aider's diff handling is well-tuned.

## Versus Claude Code alone (or Codex, Gemini alone)

AC runs Claude Code as a session inside it. So the question is: should you launch Claude Code from the terminal, or from AC?

**Use AC when**:
- You want two or more coding agents on the same repo, running in parallel.
- You want a god-view sidebar across many sessions.
- You want filesystem-based coordination between agents.
- You want voice or Telegram for hands-off operation.

**Use the CLI alone when**:
- You only ever use one coding agent and one session.
- You do not want a desktop app.

## Honest concessions

- **Windows-first.** Every release is built and tested on Windows. Linux works. macOS needs help — see [issue #320](https://github.com/mblua/AgentsCommander/issues/320).
- **You bring the coding agents.** AC does not ship Claude Code, Codex, or Gemini. Install whichever you want; AC will detect them.
- **Single-agent flows are not the sweet spot.** If you only run one agent, you are paying for a Tauri shell. The value is multi-agent coordination.

## Future positioning

A web landing page on `agentscommander.dev` is tracked in [issue #321](https://github.com/mblua/AgentsCommander/issues/321) for after the initial public push.

---

Have a comparison we should add (or a tool we should benchmark against)? Open a [GitHub Discussion](https://github.com/mblua/AgentsCommander/discussions).
