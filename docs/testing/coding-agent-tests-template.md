# Coding-agent tests template

For maintainers recording per-coding-agent compatibility evidence. After this page you can turn any coding agent's verified behavior into an auditable matrix row-by-row, using the same checklist for every agent.

This is the reusable checklist behind the per-coding-agent compatibility matrices (issue [#1545](https://github.com/mblua/AgentsCommander/issues/1545)). One matrix file per coding agent; [Antigravity (`agy`)](coding-agent-compatibility-antigravity.md) is the first filled instance. Planned files for **Claude, Codex, Pi, Hermes, Cursor CLI, and OpenCode** are not created yet — copy this checklist into each when you start the agent's file.

## How to use this template

1. Create `docs/testing/coding-agent-compatibility-<agent>.md` and copy the matrix below into it.
2. Replace the `(agent)` placeholder with the agent's id and binary, for example `Antigravity (agy)`.
3. Run each checklist item against a real agent session and fill **Result**, **Date (UTC)**, and **OS**.
4. Result is `PASS` only when an actual run observed it, and it must name the evidence kind: `live` (smoke run against a real daemon), `unit` (test suite), `manual`, or `CI`. A row without an actual run stays **PENDING** — never fill assumed results.
5. Use **Notas** for caveats, why a test failed, environment context, and anything that makes the result interpretable (for example "daemon predates merge `d64e6250` — `send_enter=false` observed").

## Checklist

The complete list of tests to run per coding agent:

1. **Agent identity/detection** — a session launched under the agent's binary is recognized as the correct `CodingAgentKind`.
2. **PTY injection wake** — `send --mode wake` to a live agent session: `send_enter=true` + `\r` dispatch, prompt executes, no stuck `waitingForInput`.
3. **Auto resume `--continue`** — restart/restore a session with resume enabled: `--continue` is auto-injected (log `Auto-injected <agent> --continue`), and the persisted `shell_args` contain no `--continue`.
4. **Resume history visibility** — after a session with **at least 4 screens of history**, restart with `--continue`: the **full history is visible** on resume (not truncated or empty).
5. **User-authored resume markers** — `-c` / `--conversation <ID>` / `--conversation=<ID>` survive persistence and suppress auto-injection.
6. **Telegram input/output** — input sent via Telegram reaches the agent PTY; agent output streams back to Telegram (requires a configured bot).
7. **Catalog preset** — the agent appears as a built-in preset with the correct command and instructions filename.
8. **Logical commands** — `/clear` and `/compact` mapping where applicable for the agent's injection profile.
9. **Transcript/JSONL watcher** — only for agents that have one (Claude, Codex). Agents without one (Antigravity, Pi) use the generic PTY path by design — record which path applies.
10. **Cross-cutting** — session create/restore/restart; fresh restart (`skip_auto_resume`); credential handling (none for most agents); update commands (none for agy).

## Matrix template

| Coding agent | Feature | Result | Date (UTC) | OS | Notas |
|---|---|---|---|---|---|
| (agent) | 1. Agent identity/detection | PENDING — no run yet | — | — | Session must be recognized as the correct `CodingAgentKind`; name the unit tests and any live session id. |
| (agent) | 2. PTY injection wake | PENDING — no run yet | — | — | Expect `[mailbox] PTY injection SUCCESS` and `[inject] PTY write OK`; record the observed `send_enter` value. |
| (agent) | 3. Auto resume `--continue` | PENDING — no run yet | — | — | Expect log `Auto-injected <agent> --continue`; persisted `shell_args` must not contain `--continue`. |
| (agent) | 4. Resume history visibility | PENDING — no run yet | — | — | Session must have ≥4 screens of history before the restart; expect the full history visible on resume. |
| (agent) | 5. User-authored resume markers | PENDING — no run yet | — | — | `-c` / `--conversation <ID>` / `--conversation=<ID>` must survive persistence and suppress auto-injection. |
| (agent) | 6. Telegram input/output | PENDING — no run yet | — | — | Requires a configured Telegram bot; verify both directions (in to PTY, out to Telegram). |
| (agent) | 7. Catalog preset | PENDING — no run yet | — | — | Record key, label, command, instructions filename, color, and whether `configSeed` / `updateCommands` exist. |
| (agent) | 8. Logical commands | PENDING — no run yet | — | — | Record the injection profile and the `/clear` / `/compact` mapping (or its absence). |
| (agent) | 9. Transcript/JSONL watcher | PENDING — no run yet | — | — | Watcher agents (Claude, Codex): name the reader and its unit tests. Others: confirm the generic PTY path (designed absence). |
| (agent) | 10. Cross-cutting | PENDING — no run yet | — | — | Cover create/restore/restart, fresh restart (`skip_auto_resume`), credential handling, and update commands. |

## See also

- [Antigravity (agy) compatibility test matrix](coding-agent-compatibility-antigravity.md) — the first filled instance of this template
- [Coding agents](../integrations/coding-agents.md) — the coding-agent catalog and tuned `CodingAgentKind` integrations
- [Regression testing](README.md) — result values, evidence rules, and the regression suite conventions
