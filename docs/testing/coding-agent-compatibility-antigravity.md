# Antigravity (agy) compatibility test matrix

For users and maintainers who need an auditable record of what was actually tested against the Antigravity CLI, when, on which OS, and with what result. After this page you know which Antigravity behaviors are verified, which are PENDING, and how to re-run the missing checks.

This is the first per-coding-agent compatibility matrix (issue [#1545](https://github.com/mblua/AgentsCommander/issues/1545)). Planned future files for **Claude, Codex, Pi, Hermes, Cursor CLI, and OpenCode** follow the same template in [coding-agent-tests-template.md](coding-agent-tests-template.md); they are not created yet.

## How to read this matrix

- **Result** is `PASS` only when an actual run observed it, and it names the evidence kind: `live` (smoke run against a real daemon), `unit` (test suite), `manual`, or `CI`. Rows without an actual run are **PENDING** — never treat a PENDING row as verified.
- **Date (UTC)** and **OS** are recorded per row. Current test host: **Windows 11 Pro, build 10.0.26200**.
- **Notas** carries the caveats, context, and failure reasons that make the result interpretable.
- The smoke-test evidence in this file ran against the daemon version merged in `d64e6250`; the live daemon at smoke time predated that merge, which matters for the `send_enter` row below.

## Antigravity (agy) matrix

| Coding agent | Feature | Result | Date (UTC) | OS | Notas |
|---|---|---|---|---|---|
| Antigravity (`agy`) | Agent identity/detection | PASS — unit: `CodingAgentKind::Antigravity` detection (exact stems `agy` / `agy.exe` / `agy.cmd` / `antigravity`, `validate_agent_commands` 16/16); live: smoke session ran under `agy` | 2026-08-24/25 (unit); 2026-08-25 (live) | Windows 11 Pro (build 10.0.26200) | Gemini no longer has tuned identity; Antigravity is a first-class kind since #1482. |
| Antigravity (`agy`) | PTY injection wake (inter-agent messaging) | PASS — live smoke: `send --mode wake` delivered `[mailbox] PTY injection SUCCESS session=464dfec1-3635-4b1c-ac4a-64cb50e1d82f msg=673f0bcc-0d29-4cb2-bd82-140c8cf20c50`, `[inject] PTY write OK session=464dfec1-… bytes=254`; prompt executed immediately, no stuck `waitingForInput` | 2026-08-25 | Windows 11 Pro (build 10.0.26200) | Caveat: the running daemon predated merge `d64e6250`, so the log shows `send_enter=false`. The `\r` dispatch path is covered by unit tests and requires the v0.30.1 daemon; see the next row. |
| Antigravity (`agy`) | send_enter / `\r` dispatch | PENDING (live) — live `send_enter=true` not observed yet; the code path `PtyInjectionProfile::Established → send_enter=true` is PASS by unit (inject suite 12/12, 2026-08-24/25) | — | — | 2026-08-25 smoke observed `send_enter=false` because the daemon predates merge `d64e6250` (binaries: `agentscommander_ac2.exe` started 2026-08-24 21:24:46 UTC, `agentscommander_testeable` 21:36:27 UTC). Re-run after a daemon restart on v0.30.1; do not claim live `send_enter=true` until that run observes it. |
| Antigravity (`agy`) | Auto resume `--continue` | PENDING (live) — unit: strip round-trip PASS (`strip_auto_injected_args` 21/21, 2026-08-24/25) | — | — | Live steps: restart/restore an agy session with resume enabled → log `Auto-injected agy --continue`; persisted `shell_args` contain no `--continue`. Needs the v0.30.1 daemon. |
| Antigravity (`agy`) | Resume history visibility | PENDING — no live run yet | — | — | User-added test (#1545): a session with **at least 4 screens of history**, restarted with `--continue`, must show the **full history** on resume (not truncated or empty). Needs a real agy session with ≥4 screens of history and the v0.30.1 daemon. |
| Antigravity (`agy`) | User-authored resume markers | PASS — unit: `-c` / `--conversation <ID>` / `--conversation=<ID>` survive persistence and suppress auto-injection (strip round-trip `preserves_user_authored_antigravity_resume_forms`, `inject_antigravity_resume_skips_when_continue_or_conversation_present`; `validate_agent_commands_allows_antigravity_conversation`); live row PENDING | 2026-08-24/25 | Windows 11 Pro (build 10.0.26200) | Live verification follows the same restart procedure as auto resume `--continue`. |
| Antigravity (`agy`) | Telegram input/output | PENDING (live) — unit: `derive_reader` returns `Ok(None)` for `CodingAgentKind::Antigravity` (generic PTY reader path) 6/6 PASS | — | — | Requires a configured Telegram bot. Verify input sent via Telegram reaches the agy PTY and agy output streams back to Telegram. |
| Antigravity (`agy`) | Catalog preset | PASS — unit: catalog tests (`embedded_default_parses_with_seven_agents_in_order`, `embedded_default_matches_current_presets_exactly`, update-commands defaults); UI seed shows the preset in Settings | 2026-08-24/25 | Windows 11 Pro (build 10.0.26200) | agy is the 7th built-in preset: key `antigravity`, label "Antigravity", description "Coding Agent by Google", color `#4285F4`, command `agy`, instructions filename `AGENTS.md`; no `configSeed`, no `updateCommands`. |
| Antigravity (`agy`) | Logical commands | PASS — unit: `PtyInjectionProfile::Established` maps `LogicalPtyCommand::Clear → /clear` and `LogicalPtyCommand::Compact → /compact` for agy stems; auto-self-maintenance and self-handoff supported (inject suite 12/12) | 2026-08-24/25 | Windows 11 Pro (build 10.0.26200) | Same mapping as Cursor (`/clear`, `/compact`); Pi differs (`/new`, no compact). |
| Antigravity (`agy`) | Transcript/JSONL watcher (absence) | PASS (designed) — unit: `derive_reader_antigravity_uses_pty_fallback_for_all_backends` (derive_reader 6/6) | 2026-08-24/25 | Windows 11 Pro (build 10.0.26200) | No transcript watcher/resolver for Antigravity; it uses the generic PTY Telegram path by design. Gemini reader/watcher wiring removed with #1482 (7 dependency arcs gone). |
| Antigravity (`agy`) | Credential handling (designed) | PASS (designed) — unit: catalog preset defaults (no `configSeed`) | 2026-08-24/25 | Windows 11 Pro (build 10.0.26200) | AC does not store coding-agent credentials. Antigravity ships no config seed and no container credential copy-in (`container_credential: None`). |
| Antigravity (`agy`) | Update commands (designed) | PASS (designed) — unit: `definition_defaults_update_commands_empty_auto_update_false_when_absent` | 2026-08-24/25 | Windows 11 Pro (build 10.0.26200) | `updateCommands` is empty for agy; `autoUpdate` defaults to `false`. Claude (`claude --update`), Pi (`pi update`), and Codex (`codex update`) ship update commands; agy does not. |
| Antigravity (`agy`) | Cross-cutting: create/restore/restart | PASS — unit: session persistence and resume-injection suites within full `cargo test` (targeted: antigravity 17, strip_auto_injected_args 21) | 2026-08-24/25 | Windows 11 Pro (build 10.0.26200) | Injection side: `inject_antigravity_resume_*` (direct agy, `cmd` tokenized wrapper, embedded `cmd` string). Persistence side: `strip_auto_injected_args_*` round trips. |
| Antigravity (`agy`) | Cross-cutting: fresh restart (`skip_auto_resume`) | PENDING — no agy-specific live run yet | — | — | Fresh restart must not auto-inject `--continue`. No dedicated live run; requires the v0.30.1 daemon. |
| Antigravity (`agy`) | Automated suite: `cargo test` (Rust) | PASS — 3842 passed / 0 failed / 25 ignored; targeted: antigravity 17, inject 12, strip_auto_injected_args 21, validate_agent_commands 16, derive_reader 6 | 2026-08-24/25 | Windows 11 Pro (build 10.0.26200) | Evidence kind: unit. Full-suite run on the Windows host. |
| Antigravity (`agy`) | Automated suite: `vitest` (frontend) | PASS — 1618 passed | 2026-08-24/25 | Windows 11 Pro (build 10.0.26200) | Evidence kind: CI. |
| Antigravity (`agy`) | Automated suite: `tsc --noEmit` | PASS — clean (no errors) | 2026-08-24/25 | Windows 11 Pro (build 10.0.26200) | Evidence kind: CI. |
| Antigravity (`agy`) | Automated suite: dependency-cycle gate | PASS — 190 modules / 3694 edges / SCC 85 members unchanged; 7 Gemini arcs removed | 2026-08-24/25 | Windows 11 Pro (build 10.0.26200) | Evidence kind: CI. The arc removal matches the Gemini wiring removal in #1482. |

## Re-running the PENDING rows

All PENDING rows share one precondition: the daemon must be the post-merge build (v0.30.1). Then, in order:

1. Restart/restore an agy session with resume enabled and check the log for `Auto-injected agy --continue`; confirm the persisted `shell_args` contain no `--continue` (auto resume `--continue`).
2. Give that session **at least 4 screens of history**, restart with `--continue`, and confirm the full history is visible (resume history visibility).
3. Send `send --mode wake` to an agy session and confirm the log shows `send_enter=true` before the `[inject]` line (send_enter / `\r` dispatch).
4. Configure a Telegram bot, attach it to an agy session, and confirm input from Telegram reaches the PTY and output streams back (Telegram input/output).

## See also

- [coding-agent-tests-template.md](coding-agent-tests-template.md) — the checklist every per-agent matrix follows
- [Coding agents](../integrations/coding-agents.md) — the coding-agent catalog and tuned `CodingAgentKind` integrations
- [Regression testing](README.md) — result values, evidence rules, and the regression suite conventions
